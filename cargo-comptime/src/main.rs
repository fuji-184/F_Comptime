use std::env;
use std::fs::{File, write};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, exit};

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

fn run_cargo_test() {
    let test_status = Command::new("cargo")
        .args(["test", "--features=comptime", "--", "--no-capture"])
        .status()
        .expect("Failed to run cargo test");
    if !test_status.success() {
        exit(test_status.code().unwrap_or(1));
    }
}

fn run_clear() {
    let clear_status = Command::new("clear").status();
    if clear_status.is_err() {
        exit(1);
    }
}

fn run_custom_commands(file_path: &str) {
    let path = Path::new(file_path);
    if !path.exists() {
        eprintln!("Error: Configuration file '{}' not found.", file_path);
        eprintln!();
        print_usage();
        exit(1);
    }
    let file = File::open(path).expect("Failed to open configuration file");
    let reader = BufReader::new(file);
    for line_result in reader.lines() {
        let line = line_result.expect("Failed to read line from configuration file");
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }
        let program = parts[0];
        let args = &parts[1..];
        let command_status = Command::new(program)
            .args(args)
            .status();
        match command_status {
            Ok(status) => {
                if !status.success() {
                    exit(status.code().unwrap_or(1));
                }
            }
            Err(_) => {
                eprintln!("Failed to run command: {}", trimmed);
                exit(1);
            }
        }
    }
}

fn handle_standard_action(action: &str, remaining_args: &[&str]) {
    run_cargo_test();
    run_clear();
    let next_status = Command::new("cargo")
        .arg(action)
        .args(remaining_args)
        .status();
    match next_status {
        Ok(status) => {
            if status.success() {
                exit(0);
            } else {
                exit(status.code().unwrap_or(1));
            }
        }
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
            eprintln!();
            print_usage();
            exit(1);
        }
    }
    match arg1 {
        "check" | "run" | "build" => {
            let remaining_args: Vec<&str> = args.iter().skip(3).map(|s| s.as_str()).collect();
            handle_standard_action(arg1, &remaining_args);
        }
        _ => {
            if arg1.starts_with('-') {
                eprintln!("Unknown option: {}", arg1);
                eprintln!();
                print_usage();
                exit(1);
            }
            run_cargo_test();
            run_clear();
            run_custom_commands(arg1);
        }
    }
}