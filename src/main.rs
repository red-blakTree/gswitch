use clap::Parser;
use gswitch::cli::Cli;

fn main() {
    // 日志级别由 RUST_LOG 环境变量控制，默认 info；
    // 调试时使用 RUST_LOG=debug 可看到详细内部状态
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format(|buf, record| {
            use std::io::Write;
            writeln!(buf, "{}", record.args())
        })
        .init();

    let cli = Cli::parse();

    // 设置 dry-run 模式
    gswitch::config::set_dry_run(cli.dry_run);
    if cli.dry_run {
        eprintln!(">>> DRY-RUN 模式：不会实际修改系统配置 <<<");
    }

    if let Err(e) = cli.run() {
        eprintln!("错误: {}", e);
        std::process::exit(1);
    }
}
