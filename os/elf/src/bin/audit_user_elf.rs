use std::{env, fs, process::ExitCode};

fn main() -> ExitCode {
    let paths: Vec<_> = env::args_os().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: audit_user_elf ELF...");
        return ExitCode::from(2);
    }
    for path in paths {
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                eprintln!(
                    "user ELF audit failed for {}: {error}",
                    path.to_string_lossy()
                );
                return ExitCode::FAILURE;
            }
        };
        if let Err(error) = elf::validate(
            &bytes,
            elf::LoadLimits {
                page_size: erhino_shared::proc::PROCESS_PAGE_SIZE as u64,
                image_limit: (erhino_shared::proc::PROCESS_USER_TOP
                    - erhino_shared::proc::PROCESS_MAIN_STACK_SIZE)
                    as u64,
            },
        ) {
            eprintln!(
                "user ELF audit failed for {}: {error:?}",
                path.to_string_lossy()
            );
            return ExitCode::FAILURE;
        }
        println!("user ELF audit passed: {}", path.to_string_lossy());
    }
    ExitCode::SUCCESS
}
