use std::env;
use std::fs::{self, File, write};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio, exit};
use std::time::{SystemTime, UNIX_EPOCH};

const CACHE_FILE: &str = "target/.comptime_last_test";

fn print_usage() {
    eprintln!("Usage:");
    eprintln!("  cargo comptime [check|run|build] [options]");
    eprintln!("  cargo comptime <path/to/comptime.config>");
    eprintln!("  cargo comptime init config");
    eprintln!("  cargo comptime -h | --help");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --release     Run cargo in release mode");
    eprintln!("  -h, --help    Show this help message");
}

fn latest_src_mtime() -> u64 {
    let mut latest = 0u64;
    let mut stack = vec!["src".to_string()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = fs::read_dir(&current) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path.to_string_lossy().to_string());
            } else if path.extension().map_or(false, |e| e == "rs") {
                if let Ok(meta) = fs::metadata(&path) {
                    if let Ok(mtime) = meta.modified() {
                        let secs = mtime.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
                        if secs > latest {
                            latest = secs;
                        }
                    }
                }
            }
        }
    }
    latest
}

fn last_test_timestamp() -> u64 {
    fs::read_to_string(CACHE_FILE)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

fn save_test_timestamp() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let _ = fs::create_dir_all("target");
    let _ = fs::write(CACHE_FILE, now.to_string());
}

fn needs_retest() -> bool {
    if latest_src_mtime() > last_test_timestamp() {
        return true;
    }
    !comptime_files_exist()
}

fn comptime_files_exist() -> bool {
    Path::new("comptime").exists()
        && fs::read_dir("comptime")
            .map(|mut d| d.next().is_some())
            .unwrap_or(false)
}

fn extract_executables(stdout: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    for line_res in BufReader::new(stdout).lines() {
        let Ok(line) = line_res else { continue };
        if !line.starts_with('{') {
            continue;
        }
        if let Some(start_idx) = line.find("\"executable\":\"") {
            let rem = &line[start_idx + 14..];
            if let Some(end_idx) = rem.find('"') {
                let path_str = &rem[..end_idx];
                if !path_str.is_empty() {
                    out.push(path_str.replace("\\\\", "\\"));
                }
            }
        }
    }
    out
}

fn run_all_test_binaries(bins: &[String]) {
    for bin_path in bins {
        let run_output = Command::new(bin_path)
            .output()
            .expect("Failed to execute test binary");
        if !run_output.status.success() {
            eprint!("{}", String::from_utf8_lossy(&run_output.stdout));
            eprint!("{}", String::from_utf8_lossy(&run_output.stderr));
            exit(1);
        }
    }
}

fn run_cargo_test() {
    regenerate_compile_time_files();
    let output = Command::new("cargo")
        .args(&["test", "--features=comptime", "--no-run", "--message-format=json", "--profile=dev", "--", "--no-capture"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .expect("Failed to compile tests");

    if !output.status.success() {
        let _ = Command::new("cargo")
            .args(&["test", "--features=comptime", "--no-run", "--profile=dev", "--", "--no-capture"])
            .status();
        exit(1);
    }

    let test_binaries = extract_executables(&output.stdout);

    if test_binaries.is_empty() {
        let _ = Command::new("cargo")
            .args(&["test", "--features=comptime", "--no-run"])
            .status();
        exit(1);
    }

    run_all_test_binaries(&test_binaries);

    save_test_timestamp();
}

fn cargo_metadata_text() -> Option<String> {
    static CACHE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| {
            let out = Command::new("cargo")
                .args(["metadata", "--format-version=1", "--no-deps"])
                .output()
                .ok()?;
            if !out.status.success() {
                return None;
            }
            String::from_utf8(out.stdout).ok()
        })
        .clone()
}

fn has_comptime_ready_feature() -> bool {
    static CACHE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CACHE.get_or_init(|| {
        let Some(text) = cargo_metadata_text() else {
            return false;
        };
        text.contains("\"comptime_ready\"")
    })
}

fn root_package_name() -> Option<String> {
    let text = cargo_metadata_text()?;
    let pkg_start = text.find("\"packages\":[")?;
    let rest = &text[pkg_start..];
    let name_pos = rest.find("\"name\":\"")? + 8;
    let end = rest[name_pos..].find('"')?;
    Some(rest[name_pos..name_pos + end].to_string())
}

fn regenerate_compile_time_files() {
    if comptime_files_exist() {
        return;
    }
    if let Some(name) = root_package_name() {
        eprintln!(
            "comptime: 'comptime/' is empty; forcing rebuild of '{}' to regenerate compile-time side effects (info files)",
            name
        );
        let _ = Command::new("cargo").args(["clean", "-p", &name]).status();
    }
}

fn phase2_cargo(extra: &[&str]) -> Command {
    let mut cmd = Command::new("cargo");
    if has_comptime_ready_feature() {
        cmd.args(["test", "--features=comptime,comptime_ready", "--no-run"]);
    } else {
        eprintln!("warning: package does not declare the `comptime_ready` feature; falling back to RUSTFLAGS (rebuilds all dependencies)");
        cmd.env("RUSTFLAGS", "--cfg comptime_ready")
            .args(["test", "--features=comptime", "--no-run"]);
    }
    cmd.args(extra);
    cmd
}

fn run_cargo_test_nested_raw() {
    regenerate_compile_time_files();
    let output = Command::new("cargo")
        .args(&["test", "--features=comptime", "--no-run", "--message-format=json", "--profile=dev", "--", "--no-capture"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .expect("Failed to compile tests");
        

    if !output.status.success() {
        let _ = Command::new("cargo")
            .args(&["test", "--features=comptime", "--no-run", "--profile=dev", "--", "--no-capture"])
            .status();
        exit(1);
    }

    let test_binaries = extract_executables(&output.stdout);

    if test_binaries.is_empty() {
        let _ = Command::new("cargo")
            .args(&["test", "--features=comptime", "--no-run"])
            .status();
        exit(1);
    }

    run_all_test_binaries(&test_binaries);
    
    let output = phase2_cargo(&["--message-format=json", "--profile=dev", "--", "--no-capture"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .expect("Failed to compile tests");
        

    if !output.status.success() {
        let _ = phase2_cargo(&["--profile=dev", "--", "--no-capture"]).status();
        exit(1);
    }

    let test_binaries = extract_executables(&output.stdout);

    if test_binaries.is_empty() {
        let _ = phase2_cargo(&["--no-run"]).status();
        exit(1);
    }

    run_all_test_binaries(&test_binaries);

    save_test_timestamp();
}

fn run_custom_commands(file_path: &str) {
    let path = Path::new(file_path);
    if !path.exists() {
        eprintln!("Error: Configuration file '{}' not found.", file_path);
        eprint!("\n");
        print_usage();
        exit(1);
    }
    let file = File::open(path).expect("Failed to open configuration file");
    let reader = BufReader::new(file);
    for line_result in reader.lines() {
        let line = line_result.expect("Failed to read line");
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }
        let status = Command::new(parts[0]).args(&parts[1..]).status();
        match status {
            Ok(s) if s.success() => {}
            Ok(s) => exit(s.code().unwrap_or(1)),
            Err(_) => {
                eprintln!("Failed to run command: {}", trimmed);
                exit(1);
            }
        }
    }
}

fn handle_standard_action(action: &str, remaining_args: &[&str]) {
    if needs_retest() {
        run_cargo_test();
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();

    let status = Command::new("cargo")
        .env("COMPTIME_NONCE", now.to_string())
        .arg(action)
        .args(remaining_args)
        .status();
    match status {
        Ok(s) => exit(s.code().unwrap_or(1)),
        Err(_) => exit(1),
    }
}

fn handle_standard_action_nested_raw(action: &str, remaining_args: &[&str]) {
    if needs_retest() {
        run_cargo_test_nested_raw();
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    let status = Command::new("cargo")
        .env("COMPTIME_NONCE", now.to_string())
        .arg(action)
        .args(remaining_args)
        .status();
    match status {
        Ok(s) => exit(s.code().unwrap_or(1)),
        Err(_) => exit(1),
    }
}

fn handle_init_config() {
    let template = "# Add your custom commands below (one per line)\n# Example\ncargo build --release\n";
    let target_path = "comptime.config";
    if Path::new(target_path).exists() {
        eprintln!("Configuration file '{}' already exists", target_path);
        exit(1);
    }
    match write(target_path, template) {
        Ok(_) => {
            println!("Created template configuration file at '{}'", target_path);
            exit(0);
        }
        Err(_) => {
            eprintln!("Failed to write configuration file");
            exit(1);
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        print_usage();
        exit(1);
    }
    let arg1 = args[2].as_str();
    if arg1 == "-h" || arg1 == "--help" {
        print_usage();
        exit(0);
    }
    if arg1 == "init" {
        if args.len() >= 4 && args[3] == "config" {
            handle_init_config();
        } else {
            eprintln!("Unknown sub-command for 'init'. Did you mean 'cargo comptime init config'?");
            eprint!("\n");
            print_usage();
            exit(1);
        }
        return;
    }
    match arg1 {
        "check" | "run" | "build" if args.len() >= 5 && args[3] == "nested" && args[4] == "raw" => {
            let remaining_args: Vec<&str> = args.iter().skip(5).map(|s| s.as_str()).collect();
            handle_standard_action_nested_raw(arg1, &remaining_args);
        }
        "check" | "run" | "build" => {
            let remaining_args: Vec<&str> = args.iter().skip(3).map(|s| s.as_str()).collect();
            handle_standard_action(arg1, &remaining_args);
        }
        _ => {
            if arg1.starts_with('-') {
                eprintln!("Unknown option: {}", arg1);
                eprint!("\n");
                print_usage();
                exit(1);
            }
            if needs_retest() {
                run_cargo_test();
            }
            run_custom_commands(arg1);
        }
    }
}