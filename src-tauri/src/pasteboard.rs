//! 系统剪贴板里的文件路径。
//!
//! 为的是"在 Finder 里 ⌘C 一个文件，回 Riot ⌘V"这条路。webview 自己的
//! 剪贴板 API 帮不上忙：`ClipboardEvent.clipboardData` 给的是 `File` 对象，
//! 出于沙箱安全**永远不带磁盘路径**。而 Riot 要的正是路径 —— 非图片文件
//! 是以 `@引用` 的形式进对话的，内容由宿主在发送那一刻按上限读（见
//! `mentions`），前端只搬路径。
//!
//! 拖放走的是另一条路（Tauri 的窗口级拖放事件，本来就给路径），不经过
//! 这里。
//!
//! macOS 读 `public.file-url`，Windows 读 `CF_HDROP` —— 两边都是文件
//! 管理器复制文件时写进粘贴板的那个格式。其它平台返回空，前端据此退回
//! "只收 webview 给的图片"的老路，而不是报错。

/// 剪贴板上现在有哪些文件（绝对路径）。没有文件就是空。
#[tauri::command]
pub fn clipboard_paths() -> Vec<String> {
    imp::clipboard_paths()
}

#[cfg(target_os = "macos")]
mod imp {
    use objc2::ClassType;
    use objc2_app_kit::NSPasteboard;
    use objc2_foundation::{NSArray, NSURL};

    pub fn clipboard_paths() -> Vec<String> {
        // 同步命令，Tauri 在主线程上跑 —— AppKit 的线程要求自动满足。
        let pb = NSPasteboard::generalPasteboard();
        let classes = NSArray::from_slice(&[<NSURL as ClassType>::class()]);

        // SAFETY: 只读。class_array 里是 NSURL，options 传 nil 合法。
        let objects = unsafe { pb.readObjectsForClasses_options(&classes, None) };
        let Some(objects) = objects else {
            return Vec::new();
        };

        objects
            .iter()
            .filter_map(|o| {
                let url = o.downcast_ref::<NSURL>()?;
                // 自己筛 isFileURL，而不是传 NSPasteboardURLReadingFileURLsOnlyKey：
                // 为一个布尔值现造一个 NSDictionary 不值当。剪贴板里躺着一条
                // http 链接时，这一筛就是它和真文件的分界。
                if !url.isFileURL() {
                    return None;
                }
                Some(url.path()?.to_string())
            })
            .collect()
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;

    use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
    use windows::Win32::System::Ole::CF_HDROP;
    use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};

    pub fn clipboard_paths() -> Vec<String> {
        // SAFETY: 开→读→关严格成对。剪贴板是全进程共享的，开了不关会让
        // 别的程序读不了剪贴板 —— 所以 read 里不允许提前 return。
        unsafe {
            if OpenClipboard(None).is_err() {
                return Vec::new();
            }
            let paths = read();
            let _ = CloseClipboard();
            paths
        }
    }

    unsafe fn read() -> Vec<String> {
        // 句柄归剪贴板所有，读完不能 DragFinish —— 那是拖放那条路才做的事。
        let Ok(handle) = (unsafe { GetClipboardData(u32::from(CF_HDROP.0)) }) else {
            return Vec::new();
        };
        let hdrop = HDROP(handle.0);

        // 0xFFFF_FFFF 是"回条数"的哨兵值，不是某个文件的下标。
        let count = unsafe { DragQueryFileW(hdrop, 0xFFFF_FFFF, None) };
        (0..count)
            .filter_map(|i| {
                // 先问长度再按长度分配:路径在某些情况下能超过 MAX_PATH，
                // 固定 260 的缓冲会把它截断成一个不存在的路径。
                let len = unsafe { DragQueryFileW(hdrop, i, None) } as usize;
                if len == 0 {
                    return None;
                }
                let mut buf = vec![0u16; len + 1]; // +1 给结尾的 NUL
                unsafe { DragQueryFileW(hdrop, i, Some(&mut buf)) };
                Some(
                    OsString::from_wide(&buf[..len])
                        .to_string_lossy()
                        .into_owned(),
                )
            })
            .collect()
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod imp {
    pub fn clipboard_paths() -> Vec<String> {
        Vec::new()
    }
}
