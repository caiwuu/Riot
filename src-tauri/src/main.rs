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
        // Windows 沙箱的 broker / runner。阶段 A 里内核嵌在宿主进程里，
        // `sandbox_win` 拿 `current_exe()` 回头调的就是这个二进制。
        //
        // `[约束]` 要把**完整**的 args_os 交给它，包括 argv[0] 和这个
        // `--srt-win` —— 它自己按 argv[1] 判断并剥掉。这条路径上不能有
        // 任何别的输出：runner 的 stdio 是 broker 的管道。
        #[cfg(windows)]
        Some(a) if a == srt_win::SRT_WIN_DISPATCH_ARG1 => {
            std::process::exit(srt_win::run_from_args(std::env::args_os()));
        }
        _ => riot_host_lib::run(),
    }
}
