//! 组态编译器：读取组态源 TOML，静态校验后写出 initfs 根的二进制组态文件。
//!
//! 用法：`formation <映像目录> <输出文件> <组态源.toml>...`；
//! `formation --list <组态源.toml>...` 在构建前列出引用映像。两者共享合并与校验。

use std::{collections::BTreeMap, fs, path::Path, process::ExitCode};

use erhino_shared::object::Rights;
use formation::{
    Budget, Formation, Input, Instance, Job, NamespaceBinding, Pool, Resource, ResourceKind,
    SERVICES_JOB, Source, right_by_name,
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
        });
    }
    let table = table(value, &context)?;
    only(table, &context, &["from", "rights"])?;
    Ok(Input {
        name: name.into(),
        source: source(string(table, "from", &context)?, &context)?,
        rights: table
            .get("rights")
            .map(|value| rights(value, &context))
            .transpose()?,
    })
}

fn namespace_binding(prefix: &str, value: &Value, context: &str) -> Result<NamespaceBinding> {
    let context = format!("{context}.namespace.\"{prefix}\"");
    let table = table(value, &context)?;
    only(table, &context, &["from", "rights"])?;
    Ok(NamespaceBinding {
        prefix: prefix.into(),
        source: source(string(table, "from", &context)?, &context)?,
        rights: rights(
            table
                .get("rights")
                .ok_or_else(|| format!("{context}: missing `rights`"))?,
            &context,
        )?,
    })
}

fn resource(name: &str, value: &Value) -> Result<Resource> {
    let context = format!("resource.{name}");
    let table = table(value, &context)?;
    let kind = string(table, "kind", &context)?;
    only(
        table,
        &context,
        match kind {
            "mailbox" => &["kind", "payer", "rights", "terminal_revoke"],
            "sender" => &[
                "kind",
                "payer",
                "rights",
                "mailbox",
                "badge",
                "terminal_revoke",
            ],
            _ => return Err(format!("{context}: unknown resource kind `{kind}`")),
        },
    )?;
    let rights = rights(
        table
            .get("rights")
            .ok_or_else(|| format!("{context}: missing `rights`"))?,
        &context,
    )?;
    let terminal_revoke = table
        .get("terminal_revoke")
        .map(|v| {
            v.as_bool()
                .ok_or_else(|| format!("{context}.terminal_revoke: expected a boolean"))
        })
        .transpose()?
        .unwrap_or(false);
    Ok(Resource {
        name: name.into(),
        payer: string(table, "payer", &context)?.into(),
        terminal_revoke,
        kind: match kind {
            "mailbox" => ResourceKind::Mailbox { rights },
            "sender" => ResourceKind::Sender {
                mailbox: string(table, "mailbox", &context)?.into(),
                badge: unsigned(table, "badge", &context)?.unwrap_or(0),
                rights,
            },
            _ => unreachable!("resource kind checked above"),
        },
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
        &["image", "job", "pool", "inputs", "namespace", "settings"],
    )?;
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
    Ok(Instance {
        name: name.into(),
        image: string(table, "image", &context)?.into(),
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
        namespace: named("namespace")?
            .into_iter()
            .map(|(prefix, value)| namespace_binding(prefix, value, &context))
            .collect::<Result<_>>()?,
        settings: settings::encode(&settings)
            .map_err(|error| format!("{context}.settings: {error:?}"))?,
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
    only(&root, "formation", &["pool", "job", "resource", "instance"])?;
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
                parent: match table.get("parent") {
                    None => SERVICES_JOB.into(),
                    Some(value) => value
                        .as_str()
                        .ok_or_else(|| format!("{context}.parent: expected a string"))?
                        .into(),
                },
                payer: string(table, "pool", &context)?.into(),
            })
        })
        .collect::<Result<_>>()?;
    let resources = section("resource")?
        .into_iter()
        .map(|(name, value)| resource(name, value))
        .collect::<Result<_>>()?;
    let instances = section("instance")?
        .into_iter()
        .map(|(name, value)| instance(name, value))
        .collect::<Result<_>>()?;
    Ok(Formation {
        pools,
        jobs,
        resources,
        instances,
    })
}

fn run(sources: &[&Path], destination: Option<(&Path, &Path)>) -> Result<()> {
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
        .validate(|component| {
            destination.is_none_or(|(images, _)| images.join(component).is_file())
        })
        .map_err(|error| error.to_string())?;
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
    let Some((_, output)) = destination else {
        for component in components {
            println!("{component}");
        }
        return Ok(());
    };
    let bytes = formation.encode().map_err(|error| error.to_string())?;
    // 写出前以 init 同一解码路径回读，编码器与解码器不一致在构建期暴露。
    if Formation::decode(&bytes).as_ref() != Ok(&formation) {
        return Err("encoded formation does not decode to the same composition".into());
    }
    fs::write(output, &bytes).map_err(|error| format!("{}: {error}", output.display()))?;
    let list = output.with_extension("images");
    fs::write(&list, components.join("\n") + "\n")
        .map_err(|error| format!("{}: {error}", list.display()))?;
    let names: Vec<&str> = formation
        .instances
        .iter()
        .map(|i| i.name.as_str())
        .collect();
    println!(
        "formation {}: {} instance(s), declaration order {}",
        output.display(),
        names.len(),
        names.join(" → ")
    );
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let (destination, sources) = match args.as_slice() {
        [_, flag, sources @ ..] if flag == "--list" => (None, sources),
        [_, images, output, sources @ ..] => {
            (Some((Path::new(images), Path::new(output))), sources)
        }
        _ => {
            eprintln!("usage: formation [--list | <image-dir> <output>] <source.toml>...");
            return ExitCode::from(2);
        }
    };
    if sources.is_empty() {
        eprintln!("usage: formation <image-dir> <output> <source.toml>...");
        return ExitCode::from(2);
    }
    let sources: Vec<&Path> = sources.iter().map(Path::new).collect();
    match run(&sources, destination) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("formation: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const SOURCE: &str = r#"
[resource.inbox]
kind="mailbox"
payer="init"
rights=["read","wait"]
[resource.entry]
kind="sender"
payer="host"
mailbox="inbox"
badge=17
rights=["write","wait","transfer","duplicate"]
terminal_revoke=true
[instance.host]
image="srv_host"
[instance.host.inputs]
receiver="init/object.inbox"
[instance.host.namespace]
"/雪"={from="init/object.entry",rights=["write","wait","transfer"]}
"#;
    fn parse(text: &str) -> Result<Formation> {
        compile(text.parse::<Table>().map_err(|e| e.to_string())?)
    }
    #[test]
    fn compiles_explicit_resources_and_namespace() {
        let f = parse(SOURCE).unwrap();
        f.validate(|_| true).unwrap();
        assert_eq!(f.resources.len(), 2);
        assert!(f.resources[1].terminal_revoke);
        assert_eq!(f.instances[0].namespace[0].prefix, "/雪");
        assert_eq!(Formation::decode(&f.encode().unwrap()).unwrap(), f);
        let f = parse(&SOURCE.replace("terminal_revoke=true", "")).unwrap();
        assert!(!f.resources[1].terminal_revoke);
    }
    #[test]
    fn system_formation_uses_current_rights_contract() {
        // 捕获正式组态保留旧权利名；不声称实际映像已构建。
        let f = parse(include_str!("../../../user/formations/system.toml")).unwrap();
        f.validate(|_| true).unwrap();
        assert_eq!(Formation::decode(&f.encode().unwrap()).unwrap(), f);
    }

    #[test]
    fn legacy_fields_and_export_routes_are_not_adapted() {
        for source in [
            "finish='stay'",
            "[instance.x]\nimage='srv_x'\nkind='resident'",
            "[instance.x]\nimage='srv_x'\nafter=[]",
            "[instance.x]\nimage='srv_x'\nstart_timeout_ms=1",
            "[instance.x]\nimage='srv_x'\nstop_timeout_ms=1",
            "[instance.x.exports.a]\nkind='mailbox_sender'\nrights=['write']",
            "[instance.x.inputs.a]\nfrom='init/platform.root'\nrevoke='init/object.admin'",
        ] {
            assert!(parse(source).is_err(), "accepted old field: {source}");
        }
        assert!(parse(&SOURCE.replace("init/object.inbox", "provider/inbox")).is_err());
    }
    #[test]
    fn malformed_resource_fields_and_rights_rejected() {
        for source in [
            SOURCE.replace("payer=\"init\"", ""),
            SOURCE.replace("terminal_revoke=true", "terminal_revoke='true'"),
            SOURCE.replace("kind=\"sender\"", "kind=\"notification\""),
            SOURCE.replace("badge=17", "badge=-1"),
            SOURCE.replace("rights=[\"read\",\"wait\"]", "rights=[\"unknown\"]"),
            SOURCE.replace("transfer", "transit"),
            SOURCE.replace("transfer", "grant"),
        ] {
            assert!(parse(&source).is_err());
        }
    }
    #[test]
    fn layering_preserves_declaration_order_and_overrides_only_named_values() {
        let mut base: Table = SOURCE.parse().unwrap();
        merge(
            &mut base,
            "[resource.entry]\nbadge=23\n[instance.other]\nimage='srv_other'"
                .parse()
                .unwrap(),
        );
        let f = compile(base).unwrap();
        f.validate(|_| true).unwrap();
        assert_eq!(
            f.instances
                .iter()
                .map(|i| i.name.as_str())
                .collect::<Vec<_>>(),
            ["host", "other"]
        );
        assert!(matches!(
            f.resources[1].kind,
            ResourceKind::Sender { badge: 23, .. }
        ));
    }
}
