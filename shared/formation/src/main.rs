//! 组态编译器：读取组态源 TOML，静态校验后写出 initfs 根的二进制组态文件。
//!
//! 用法：`formation <组态源.toml> <映像目录> <输出文件>`。映像目录即 initfs 的 `image/`，
//! 用于核对引用的组件映像存在。未知键一律拒绝，避免组态笔误被静默忽略。

use std::{collections::BTreeMap, fs, path::Path, process::ExitCode};

use erhino_shared::object::Rights;
use formation::{
    Budget, Export, Finish, Formation, Input, Instance, Job, Kind, Pool, SERVICES_JOB, Source,
    kind_by_name, right_by_name,
};
use toml::{Table, Value};

type Result<T> = std::result::Result<T, String>;

fn only(table: &Table, context: &str, known: &[&str]) -> Result<()> {
    match table.keys().find(|key| !known.contains(&key.as_str())) {
        Some(key) => Err(format!("{context}: unknown key `{key}`")),
        None => Ok(()),
    }
}

fn table<'a>(value: &'a Value, context: &str) -> Result<&'a Table> {
    value
        .as_table()
        .ok_or_else(|| format!("{context}: expected a table"))
}

fn string<'a>(table: &'a Table, key: &str, context: &str) -> Result<&'a str> {
    table
        .get(key)
        .ok_or_else(|| format!("{context}: missing `{key}`"))?
        .as_str()
        .ok_or_else(|| format!("{context}.{key}: expected a string"))
}

fn unsigned(table: &Table, key: &str, context: &str) -> Result<Option<u64>> {
    match table.get(key) {
        None => Ok(None),
        Some(value) => value
            .as_integer()
            .and_then(|value| u64::try_from(value).ok())
            .map(Some)
            .ok_or_else(|| format!("{context}.{key}: expected a non-negative integer")),
    }
}

fn budget(value: &Value, context: &str) -> Result<Budget> {
    let table = table(value, context)?;
    only(table, context, &["work_pages", "metadata_bytes"])?;
    Ok(Budget {
        work_pages: unsigned(table, "work_pages", context)?.unwrap_or(0),
        metadata_bytes: unsigned(table, "metadata_bytes", context)?.unwrap_or(0),
    })
}

fn source(text: &str, context: &str) -> Result<Source> {
    Source::parse(text).ok_or_else(|| format!("{context}: invalid source `{text}`"))
}

fn rights(value: &Value, context: &str) -> Result<Rights> {
    let items = value
        .as_array()
        .ok_or_else(|| format!("{context}: expected an array of rights"))?;
    let mut rights = Rights::NONE;
    for item in items {
        let name = item
            .as_str()
            .ok_or_else(|| format!("{context}: right names are strings"))?;
        rights |=
            right_by_name(name).ok_or_else(|| format!("{context}: unknown right `{name}`"))?;
    }
    Ok(rights)
}

fn input(name: &str, value: &Value, context: &str) -> Result<Input> {
    let context = format!("{context}.inputs.\"{name}\"");
    if let Some(text) = value.as_str() {
        return Ok(Input {
            name: name.into(),
            source: source(text, &context)?,
            rights: None,
            revoke: None,
        });
    }
    let table = table(value, &context)?;
    only(table, &context, &["from", "rights", "revoke"])?;
    Ok(Input {
        name: name.into(),
        source: source(string(table, "from", &context)?, &context)?,
        rights: table
            .get("rights")
            .map(|value| rights(value, &context))
            .transpose()?,
        revoke: table
            .get("revoke")
            .map(|value| {
                value
                    .as_str()
                    .ok_or_else(|| format!("{context}.revoke: expected a string"))
                    .and_then(|text| source(text, &context))
            })
            .transpose()?,
    })
}

fn export(name: &str, value: &Value, context: &str) -> Result<Export> {
    let context = format!("{context}.exports.\"{name}\"");
    let table = table(value, &context)?;
    only(table, &context, &["kind", "rights"])?;
    let kind = string(table, "kind", &context)?;
    Ok(Export {
        name: name.into(),
        kind: kind_by_name(kind).ok_or_else(|| format!("{context}: unknown kind `{kind}`"))?,
        rights: rights(
            table
                .get("rights")
                .ok_or_else(|| format!("{context}: missing `rights`"))?,
            &context,
        )?,
    })
}

/// TOML 值到实例设定；浮点与日期没有对应的设定类型。
fn setting(value: &Value, context: &str) -> Result<settings::Owned> {
    Ok(match value {
        Value::Boolean(value) => settings::Owned::Bool(*value),
        Value::Integer(value) => settings::Owned::Int(*value),
        Value::String(value) => settings::Owned::Str(value.clone()),
        Value::Array(items) => settings::Owned::List(
            items
                .iter()
                .map(|item| setting(item, context))
                .collect::<Result<_>>()?,
        ),
        Value::Table(entries) => settings::Owned::Table(setting_table(entries, context)?),
        Value::Float(_) | Value::Datetime(_) => {
            return Err(format!("{context}: settings support no floats or dates"));
        }
    })
}

fn setting_table(entries: &Table, context: &str) -> Result<BTreeMap<String, settings::Owned>> {
    entries
        .iter()
        .map(|(key, value)| Ok((key.clone(), setting(value, &format!("{context}.{key}"))?)))
        .collect()
}

fn instance(name: &str, value: &Value) -> Result<Instance> {
    let context = format!("instance.{name}");
    let table = table(value, &context)?;
    only(
        table,
        &context,
        &[
            "image",
            "kind",
            "job",
            "pool",
            "inputs",
            "exports",
            "settings",
            "start_timeout_ms",
            "stop_timeout_ms",
            "after",
        ],
    )?;
    let kind = match string(table, "kind", &context)? {
        "resident" => Kind::Resident,
        "task" => Kind::Task,
        other => return Err(format!("{context}.kind: unknown kind `{other}`")),
    };
    let named = |key: &str| -> Result<Vec<(&String, &Value)>> {
        match table.get(key) {
            None => Ok(Vec::new()),
            Some(value) => Ok(self::table(value, &format!("{context}.{key}"))?
                .iter()
                .collect()),
        }
    };
    let settings = match table.get("settings") {
        None => BTreeMap::new(),
        Some(value) => setting_table(
            self::table(value, &format!("{context}.settings"))?,
            &format!("{context}.settings"),
        )?,
    };
    let after = match table.get("after") {
        None => Vec::new(),
        Some(value) => value
            .as_array()
            .ok_or_else(|| format!("{context}.after: expected an array"))?
            .iter()
            .map(|item| {
                item.as_str()
                    .map(String::from)
                    .ok_or_else(|| format!("{context}.after: instance names are strings"))
            })
            .collect::<Result<_>>()?,
    };
    Ok(Instance {
        name: name.into(),
        image: string(table, "image", &context)?.into(),
        kind,
        job: table
            .get("job")
            .map(|value| {
                value
                    .as_str()
                    .map(String::from)
                    .ok_or_else(|| format!("{context}.job: expected a string"))
            })
            .transpose()?,
        pool: table
            .get("pool")
            .map(|value| budget(value, &format!("{context}.pool")))
            .transpose()?
            .unwrap_or_default(),
        inputs: named("inputs")?
            .into_iter()
            .map(|(name, value)| input(name, value, &context))
            .collect::<Result<_>>()?,
        exports: named("exports")?
            .into_iter()
            .map(|(name, value)| export(name, value, &context))
            .collect::<Result<_>>()?,
        settings: settings::encode(&settings)
            .map_err(|error| format!("{context}.settings: {error:?}"))?,
        start_timeout_ms: unsigned(table, "start_timeout_ms", &context)?.unwrap_or(0),
        stop_timeout_ms: unsigned(table, "stop_timeout_ms", &context)?.unwrap_or(0),
        after,
    })
}

/// 后层覆盖前层：表逐键递归合并，其余值整体替换。部署由基础组态加覆盖层组成。
fn merge(base: &mut Table, layer: Table) {
    for (key, value) in layer {
        match (base.get_mut(&key), value) {
            (Some(Value::Table(existing)), Value::Table(table)) => merge(existing, table),
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

fn compile(root: Table) -> Result<Formation> {
    only(&root, "formation", &["finish", "pool", "job", "instance"])?;
    let finish = match string(&root, "finish", "formation")? {
        "stay" => Finish::Stay,
        "shutdown" => Finish::Shutdown,
        "reboot" => Finish::Reboot,
        other => return Err(format!("formation.finish: unknown action `{other}`")),
    };
    let section = |key: &str| -> Result<Vec<(&String, &Value)>> {
        match root.get(key) {
            None => Ok(Vec::new()),
            Some(value) => Ok(table(value, key)?.iter().collect()),
        }
    };
    let pools = section("pool")?
        .into_iter()
        .map(|(name, value)| {
            Ok(Pool {
                name: name.clone(),
                budget: budget(value, &format!("pool.{name}"))?,
            })
        })
        .collect::<Result<_>>()?;
    let jobs = section("job")?
        .into_iter()
        .map(|(name, value)| {
            let context = format!("job.{name}");
            let table = table(value, &context)?;
            only(table, &context, &["parent", "pool"])?;
            Ok(Job {
                name: name.clone(),
                parent: table
                    .get("parent")
                    .and_then(Value::as_str)
                    .unwrap_or(SERVICES_JOB)
                    .into(),
                payer: string(table, "pool", &context)?.into(),
            })
        })
        .collect::<Result<_>>()?;
    let instances = section("instance")?
        .into_iter()
        .map(|(name, value)| instance(name, value))
        .collect::<Result<_>>()?;
    Ok(Formation {
        finish,
        pools,
        jobs,
        instances,
    })
}

fn run(sources: &[&Path], images: &Path, output: &Path) -> Result<()> {
    let mut root = Table::new();
    for source in sources {
        let text =
            fs::read_to_string(source).map_err(|error| format!("{}: {error}", source.display()))?;
        let layer: Table = text
            .parse()
            .map_err(|error| format!("{}: {error}", source.display()))?;
        merge(&mut root, layer);
    }
    let formation = compile(root)?;
    formation
        .validate(|component| images.join(component).is_file())
        .map_err(|error| error.to_string())?;
    let bytes = formation.encode().map_err(|error| error.to_string())?;
    // 写出前以 init 同一解码路径回读，编码器与解码器不一致在构建期暴露。
    if Formation::decode(&bytes).as_ref() != Ok(&formation) {
        return Err("encoded formation does not decode to the same composition".into());
    }
    fs::write(output, &bytes).map_err(|error| format!("{}: {error}", output.display()))?;
    // 构建按此清单只把组态引用的映像放入 initfs。
    let mut components: Vec<&str> = formation
        .instances
        .iter()
        .map(|instance| instance.image.as_str())
        .chain(formation.instances.iter().flat_map(|instance| {
            instance
                .inputs
                .iter()
                .filter_map(|input| match &input.source {
                    Source::Image(component) => Some(component.as_str()),
                    _ => None,
                })
        }))
        .collect();
    components.sort_unstable();
    components.dedup();
    let list = output.with_extension("images");
    fs::write(&list, components.join("\n") + "\n")
        .map_err(|error| format!("{}: {error}", list.display()))?;
    let order = formation.order().map_err(|error| error.to_string())?;
    let names: Vec<&str> = order
        .iter()
        .map(|&index| formation.instances[index].name.as_str())
        .collect();
    println!(
        "formation {}: {} instance(s), start order {}",
        output.display(),
        names.len(),
        names.join(" → ")
    );
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let [_, images, output, sources @ ..] = args.as_slice() else {
        eprintln!("usage: formation <image-dir> <output> <source.toml>...");
        return ExitCode::from(2);
    };
    if sources.is_empty() {
        eprintln!("usage: formation <image-dir> <output> <source.toml>...");
        return ExitCode::from(2);
    }
    let sources: Vec<&Path> = sources.iter().map(Path::new).collect();
    match run(&sources, Path::new(images), Path::new(output)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("formation: {error}");
            ExitCode::FAILURE
        }
    }
}
