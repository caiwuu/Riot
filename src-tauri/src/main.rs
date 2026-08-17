// Windows release 构建不要弹控制台窗口。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let mut args = std::env::args();
    let _ = args.next();
    match args.next().as_deref() {
        // 登录 shell 里再 exec 一次，把算出来的环境打成 JSON。必须在
        // run() 之前返回，否则会递归吸入、再起一个窗口。
        Some("--print-env") => riot_host_lib::print_process_env(),
        // git / ssh 的 askpass 助手。参数是提示词，答案打到 stdout。
        Some("--askpass") => {
            let prompt = args.collect::<Vec<_>>().join(" ");
            std::process::exit(riot_host_lib::run_askpass(&prompt));
        }
        _ => riot_host_lib::run(),
    }
}
