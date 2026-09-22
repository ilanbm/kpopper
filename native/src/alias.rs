//! The long CLI spelling forwards to the same canonical executable and identity.
use std::process::{Command, exit};

fn main() {
    let result = (|| -> std::io::Result<i32> {
        let executable = std::env::current_exe()?;
        let name = if cfg!(windows) { "kpop.exe" } else { "kpop" };
        let target = executable.with_file_name(name);
        let mut command = Command::new(target);
        command.args(std::env::args_os().skip(1));
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            Err(command.exec())
        }
        #[cfg(not(unix))]
        {
            Ok(command.status()?.code().unwrap_or(1))
        }
    })();
    match result {
        Ok(code) => exit(code),
        Err(error) => {
            eprintln!("kpopper: could not start the adjacent kpop executable: {error}");
            exit(1);
        }
    }
}
