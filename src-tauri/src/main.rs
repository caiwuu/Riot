// Windows release 构建不要弹控制台窗口。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    riot_host_lib::run()
}
