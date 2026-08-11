//! 可脚本化的子进程替身。
//!
//! 除了产出预设的输出，它还**记下每次收到的 [`ProcessSpec`]**。这部分同样
//! 重要：Bash 工具正确与否有一半在于它怎么起这个进程 —— 环境变量对不对、
//! 有没有误加 `-l`、cwd 是不是会话目录。这些东西不看 spec 就测不到。

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use riot_protocol::tool::{ProcessOutput, ProcessRunner, ProcessSpec};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub enum Script {
    /// 正常结束。
    Exit {
        stdout: String,
        stderr: String,
        code: i32,
    },
    /// 跑到超时被杀，带上超时前已经产出的输出。
    Timeout { stdout: String, stderr: String },
    /// 起不来。
    Spawn(std::io::ErrorKind),
}

impl Script {
    pub fn ok(stdout: &str) -> Self {
        Script::Exit {
            stdout: stdout.to_owned(),
            stderr: String::new(),
            code: 0,
        }
    }

    pub fn fail(code: i32, stderr: &str) -> Self {
        Script::Exit {
            stdout: String::new(),
            stderr: stderr.to_owned(),
            code,
        }
    }
}

#[derive(Default)]
pub struct FakeProc {
    /// 命令原文 → 脚本。
    scripts: Mutex<HashMap<String, Script>>,
    /// 命令原文没命中时用这个。
    fallback: Mutex<Option<Script>>,
    seen: Mutex<Vec<ProcessSpec>>,
}

impl FakeProc {
    pub fn new() -> Self {
        Self::default()
    }

    /// 给某条命令配一个结果。key 是 `bash -c` 后面那串原文。
    pub fn on(self, command: &str, script: Script) -> Self {
        self.scripts
            .lock()
            .expect("锁未中毒")
            .insert(command.to_owned(), script);
        self
    }

    pub fn default_script(self, script: Script) -> Self {
        *self.fallback.lock().expect("锁未中毒") = Some(script);
        self
    }

    /// 最后一次收到的 spec。
    pub fn last_spec(&self) -> Option<ProcessSpec> {
        self.seen.lock().expect("锁未中毒").last().cloned()
    }

    pub fn call_count(&self) -> usize {
        self.seen.lock().expect("锁未中毒").len()
    }
}

#[async_trait]
impl ProcessRunner for FakeProc {
    async fn run(
        &self,
        spec: ProcessSpec,
        _cancel: CancellationToken,
    ) -> std::io::Result<ProcessOutput> {
        let command = spec.args.last().cloned().unwrap_or_default();
        self.seen.lock().expect("锁未中毒").push(spec);

        let script = self
            .scripts
            .lock()
            .expect("锁未中毒")
            .get(&command)
            .cloned()
            .or_else(|| self.fallback.lock().expect("锁未中毒").clone());

        match script {
            Some(Script::Exit {
                stdout,
                stderr,
                code,
            }) => Ok(ProcessOutput {
                stdout,
                stderr,
                exit_code: code,
                timed_out: false,
                duration_ms: 12,
            }),
            Some(Script::Timeout { stdout, stderr }) => Ok(ProcessOutput {
                stdout,
                stderr,
                // 被信号杀掉时 shell 的惯例退出码
                exit_code: 143,
                timed_out: true,
                duration_ms: 120_000,
            }),
            Some(Script::Spawn(kind)) => Err(std::io::Error::new(kind, "起不来")),
            // 没配脚本就是测试写漏了。返回一个"成功"会让断言在
            // 错误的前提下通过 —— 那种绿灯毫无意义。
            None => Err(std::io::Error::other(format!(
                "FakeProc 没有为 `{command}` 配置脚本"
            ))),
        }
    }
}
