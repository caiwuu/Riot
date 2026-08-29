import { useEffect, useState } from "react";

import {
  type AppConfig,
  type ConfigStatus,
  type McpServerConfig,
  type McpServerStatus,
  mcpExportJson,
  mcpImportJson,
  mcpRestart,
  mcpStatus,
  setConfig,
} from "../../bridge";
import { ResizableTextarea } from "../ResizableTextarea";
import { Card, CardBlock, Group, Row } from "./layout";
import { type AskConfirm, type LeaveGuard, FormError, Switch, blurOnEnter } from "./shared";

/**
 * MCP 服务器管理。
 *
 * 配置改动走 setConfig（宿主保存后自动 reconcile 连接）；连接状态另有
 * 一条只读通道（mcpStatus），打开本页时轮询 —— 配置是"想要什么"，
 * 状态是"现在是什么"，两者永远可能不一致（正在连、连失败了）。
 */
export function McpPane({
  status,
  onStatus,
  askConfirm,
  registerLeaveGuard,
  onSaved,
}: {
  status: ConfigStatus;
  onStatus: (s: ConfigStatus) => void;
  askConfirm: AskConfirm;
  registerLeaveGuard: (g: LeaveGuard | null) => void;
  onSaved: () => void;
}) {
  const cfg = status.config;
  const servers = cfg.mcpServers;
  const [selId, setSelId] = useState(servers[0]?.id ?? "");
  /** 刚新建的服务器：编辑器聚焦到名称，省得用户自己找第一个待填字段。 */
  const [justAdded, setJustAdded] = useState("");
  const [error, setError] = useState("");
  const [live, setLive] = useState<McpServerStatus[]>([]);
  /** null = 表单视图；字符串 = JSON 视图的编辑内容。 */
  const [jsonDraft, setJsonDraft] = useState<string | null>(null);
  /** 打开 JSON 视图那一刻的导出值。和它相同 = 没改过，关掉不用问。 */
  const [jsonBase, setJsonBase] = useState("");
  const [jsonBusy, setJsonBusy] = useState(false);

  const sel = servers.find((s) => s.id === selId) ?? servers[0];

  // 用户粘了一整段 JSON、还没保存 —— 关设置或切分区前得先问一句。
  // 这可能是他花了几分钟从 README 里拼出来的东西。
  const jsonDirty = jsonDraft !== null && jsonDraft !== jsonBase;
  useEffect(() => {
    registerLeaveGuard(
      jsonDirty
        ? () => ({
            title: "放弃未保存的 JSON 配置？",
            body: "JSON 视图里的改动还没保存，离开会丢掉它们。",
            confirmLabel: "放弃",
          })
        : null,
    );
    return () => registerLeaveGuard(null);
  }, [jsonDirty, registerLeaveGuard]);

  const openJson = async () => {
    setError("");
    try {
      const s = await mcpExportJson();
      setJsonBase(s);
      setJsonDraft(s);
    } catch (e) {
      setError(String(e));
    }
  };

  const applyJson = async () => {
    if (jsonDraft === null) return;
    setJsonBusy(true);
    setError("");
    try {
      const s = await mcpImportJson(jsonDraft);
      onStatus(s);
      onSaved();
      setJsonDraft(null);
      setSelId(s.config.mcpServers[0]?.id ?? "");
    } catch (e) {
      // 留在 JSON 视图里报错 —— 关掉的话用户就丢了刚粘的内容
      setError(String(e));
    } finally {
      setJsonBusy(false);
    }
  };

  // 打开本页时轮询连接状态。2.5s 是"点了重连能很快看到变化"和
  // "别对着宿主刷屏"之间的折中。
  useEffect(() => {
    let stopped = false;
    const pull = async () => {
      try {
        const s = await mcpStatus();
        if (!stopped) setLive(s);
      } catch {
        // 拿不到状态就保持上一份 —— 状态点短暂过时无伤大雅
      }
    };
    void pull();
    const timer = setInterval(() => void pull(), 2500);
    return () => {
      stopped = true;
      clearInterval(timer);
    };
  }, []);

  const commit = async (next: AppConfig) => {
    setError("");
    try {
      onStatus(await setConfig(next));
      onSaved();
      return true;
    } catch (e) {
      setError(String(e));
      return false;
    }
  };

  const patchSel = (patch: Partial<McpServerConfig>) => {
    if (!sel) return;
    void commit({
      ...cfg,
      mcpServers: servers.map((s) => (s.id === sel.id ? { ...s, ...patch } : s)),
    });
  };

  const addServer = () => {
    // id 是工具名和权限规则的一部分，给一个能用但明显该改的默认值。
    let n = servers.length + 1;
    while (servers.some((s) => s.id === `server-${n}`)) n += 1;
    const id = `server-${n}`;
    const s: McpServerConfig = { id, command: "", args: [], env: {}, enabled: true };
    void commit({ ...cfg, mcpServers: [...servers, s] }).then((ok) => {
      if (ok) {
        setSelId(id);
        setJustAdded(id);
      }
    });
  };

  const removeServer = () => {
    if (!sel) return;
    const target = sel;
    askConfirm({
      title: `删除 MCP 服务器「${target.name || target.id}」？`,
      body: "它的进程会被停掉，工具在下一轮对话消失。",
      confirmLabel: "删除",
      action: () => {
        const rest = servers.filter((s) => s.id !== target.id);
        void commit({ ...cfg, mcpServers: rest }).then((ok) => {
          if (ok) setSelId(rest[0]?.id ?? "");
        });
      },
    });
  };

  // JSON 视图：显示并编辑标准格式（{"mcpServers": {...}}），整体替换。
  if (jsonDraft !== null) {
    return (
      <Group
        title="JSON 配置"
        desc={
          <>
            标准格式，和 Claude Desktop / Cursor / Cline 通用。README 里的{" "}
            <code>mcpServers</code> 片段可以整段粘贴。保存会整体替换当前列表。
          </>
        }
      >
        <ResizableTextarea
          className="mcp-json-input"
          value={jsonDraft}
          onChange={(e) => setJsonDraft(e.target.value)}
          rows={16}
          spellCheck={false}
        />
        <div className="editor-foot">
          {error ? <span className="test-result err">{error}</span> : <span />}
          <div className="editor-foot-actions">
            <button
              onClick={() => {
                const back = () => {
                  setJsonDraft(null);
                  setError("");
                };
                if (jsonDirty) {
                  askConfirm({
                    title: "放弃未保存的 JSON 配置？",
                    body: "改动还没保存，返回表单视图会丢掉它们。",
                    confirmLabel: "放弃",
                    action: back,
                  });
                } else {
                  back();
                }
              }}
              disabled={jsonBusy}
            >
              取消
            </button>
            <button className="primary" onClick={() => void applyJson()} disabled={jsonBusy}>
              {jsonBusy ? "保存中…" : "保存"}
            </button>
          </div>
        </div>
      </Group>
    );
  }

  if (!sel) {
    return (
      <Group title="服务器">
        <div className="empty-state">
          <p className="empty-title">还没有 MCP 服务器</p>
          <p className="hint">
            通过 MCP 接入外部工具（文件、数据库、API……）。服务器跑在本机的
            独立进程里，工具和内置工具走同一套权限询问。
          </p>
          <div className="empty-actions">
            <button className="primary" onClick={addServer}>
              添加服务器
            </button>
            <button onClick={() => void openJson()}>粘贴 JSON 配置</button>
          </div>
          {error ? <p className="form-error">{error}</p> : null}
        </div>
      </Group>
    );
  }

  return (
    <>
      <Group
        title="服务器"
        desc={
          <>
            工具名是 <code>mcp__服务器id__…</code>
            ，权限规则按它匹配。每个工具首次调用会像内置工具一样询问。
          </>
        }
        action={
          <div className="set-group-actions">
            <button className="btn-compact" onClick={addServer}>
              添加
            </button>
            <button
              className="btn-compact"
              onClick={() => void openJson()}
              title="以标准 JSON 格式查看和编辑"
            >
              JSON
            </button>
          </div>
        }
      >
        {/* 竖排列表而不是 pill 铺排：几十个服务器时 pill 会糊成一片，
            长名字还会把整行撑爆。行内名字省略号截断，超高滚动。 */}
        <ul className="mcp-list">
          {servers.map((s) => {
            const st = live.find((l) => l.id === s.id);
            const state = s.enabled === false ? "off" : (st?.state ?? "off");
            const meta =
              s.enabled === false
                ? "已停用"
                : state === "connected"
                  ? `${st?.tools.length ?? 0} 个工具`
                  : state === "connecting"
                    ? "连接中…"
                    : state === "failed"
                      ? "连接失败"
                      : "";
            return (
              <li key={s.id}>
                <button
                  className={s.id === sel.id ? "mcp-row active" : "mcp-row"}
                  onClick={() => {
                    setSelId(s.id);
                    setJustAdded("");
                  }}
                  title={s.name || s.id}
                >
                  <span className={`mcp-dot ${state}`} />
                  <span className="mcp-row-name">{s.name || s.id}</span>
                  {meta ? <span className="mcp-row-meta">{meta}</span> : null}
                </button>
              </li>
            );
          })}
        </ul>
      </Group>

      <McpServerEditor
        key={sel.id}
        server={sel}
        live={live.find((l) => l.id === sel.id) ?? null}
        autoFocusName={sel.id === justAdded}
        onPatch={patchSel}
        onRemove={removeServer}
        onError={setError}
      />

      {error ? <FormError text={error} /> : null}
    </>
  );
}

/** 已连接服务器的工具名默认只铺这么多个，其余收起 —— 几十个全量平铺会把配置区挤到两屏外。 */
const MCP_TOOLS_SHOWN = 12;

/** `mcp__better-icons__search_icons` → `search_icons`。前缀每条都一样，铺出来只添噪音。 */
function mcpToolShortName(full: string): string {
  const parts = full.split("__");
  if (parts[0] === "mcp" && parts.length >= 3) return parts.slice(2).join("__");
  return full;
}

function McpServerEditor({
  server,
  live,
  autoFocusName,
  onPatch,
  onRemove,
  onError,
}: {
  server: McpServerConfig;
  live: McpServerStatus | null;
  /** 刚新建时聚焦名称输入框。 */
  autoFocusName?: boolean;
  onPatch: (p: Partial<McpServerConfig>) => void;
  onRemove: () => void;
  onError: (e: string) => void;
}) {
  const [name, setName] = useState(server.name ?? "");
  const [command, setCommand] = useState(server.command);
  const [args, setArgs] = useState((server.args ?? []).join("\n"));
  const [env, setEnv] = useState(
    Object.entries(server.env ?? {})
      .map(([k, v]) => `${k}=${v}`)
      .join("\n"),
  );
  const [restarting, setRestarting] = useState(false);
  const [toolsOpen, setToolsOpen] = useState(false);

  const commitArgs = () => {
    const list = args
      .split("\n")
      .map((a) => a.trim())
      .filter(Boolean);
    if (JSON.stringify(list) !== JSON.stringify(server.args ?? [])) onPatch({ args: list });
  };

  const commitEnv = () => {
    const map: Record<string, string> = {};
    for (const line of env.split("\n")) {
      const t = line.trim();
      if (!t) continue;
      const eq = t.indexOf("=");
      if (eq <= 0) {
        onError(`环境变量要写成 KEY=VALUE：「${t}」`);
        return;
      }
      map[t.slice(0, eq).trim()] = t.slice(eq + 1).trim();
    }
    if (JSON.stringify(map) !== JSON.stringify(server.env ?? {})) onPatch({ env: map });
  };

  const doRestart = async () => {
    setRestarting(true);
    try {
      await mcpRestart(server.id);
    } catch (e) {
      onError(String(e));
    } finally {
      setRestarting(false);
    }
  };

  const state = server.enabled === false ? "off" : (live?.state ?? "off");
  const stateText: Record<string, string> = {
    connected: `已连接${live?.detail ? ` · ${live.detail}` : ""} · ${live?.tools.length ?? 0} 个工具`,
    connecting: "连接中…",
    failed: `连接失败：${live?.detail ?? "未知原因"}`,
    off:
      server.enabled === false
        ? "已停用"
        : server.command.trim()
          ? "未启动（保存配置后自动连接）"
          : "填好启动命令后自动连接",
  };

  return (
    <Group
      title={server.name || server.id}
      action={
        <div className="set-group-actions">
          <button className="btn-compact ghost-danger" onClick={onRemove}>
            删除服务器
          </button>
        </div>
      }
    >
      <Card>
        <Row title="启用" desc="关掉后进程会停，它的工具在下一轮对话里消失。">
          <Switch
            on={server.enabled !== false}
            onChange={(v) => onPatch({ enabled: v })}
            label="启用这个服务器"
          />
        </Row>
        <CardBlock>
          <div className={`mcp-status ${state}`}>
            <span className={`mcp-dot ${state}`} />
            <span className="mcp-status-text">{stateText[state]}</span>
            {server.enabled !== false ? (
              <button className="ghost" onClick={() => void doRestart()} disabled={restarting}>
                {restarting ? "重连中…" : "重连"}
              </button>
            ) : null}
          </div>
          {state === "connected" && live && live.tools.length > 0 ? (
            <ul className="mcp-tools">
              {(toolsOpen ? live.tools : live.tools.slice(0, MCP_TOOLS_SHOWN)).map((t) => (
                <li key={t}>
                  <span className="mcp-tool-chip" title={t}>
                    {mcpToolShortName(t)}
                  </span>
                </li>
              ))}
              {live.tools.length > MCP_TOOLS_SHOWN ? (
                <li>
                  <button
                    type="button"
                    className="mcp-tools-more"
                    onClick={() => setToolsOpen(!toolsOpen)}
                  >
                    {toolsOpen ? "收起" : `还有 ${live.tools.length - MCP_TOOLS_SHOWN} 个`}
                  </button>
                </li>
              ) : null}
            </ul>
          ) : null}
        </CardBlock>
        <Row title="名称" desc="只在界面上显示。工具名用的是服务器 id。">
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            onBlur={() => name.trim() !== (server.name ?? "") && onPatch({ name: name.trim() })}
            onKeyDown={blurOnEnter}
            autoFocus={autoFocusName}
            placeholder={server.id}
            spellCheck={false}
            aria-label="名称"
          />
        </Row>
        <Row title="启动命令">
          <input
            value={command}
            onChange={(e) => setCommand(e.target.value)}
            onBlur={() => command.trim() !== server.command && onPatch({ command: command.trim() })}
            onKeyDown={blurOnEnter}
            placeholder="npx / uvx / 可执行文件路径"
            spellCheck={false}
            aria-label="启动命令"
          />
        </Row>
        <Row title="参数" desc="一行一个。" stack>
          <ResizableTextarea
            className="paths-input"
            value={args}
            onChange={(e) => setArgs(e.target.value)}
            onBlur={commitArgs}
            placeholder={"如：\n-y\n@modelcontextprotocol/server-filesystem\n/tmp"}
            rows={4}
            spellCheck={false}
            aria-label="参数"
          />
        </Row>
        <Row title="环境变量" desc="一行一个 KEY=VALUE。" stack>
          <ResizableTextarea
            className="paths-input"
            value={env}
            onChange={(e) => setEnv(e.target.value)}
            onBlur={commitEnv}
            placeholder={"如：\nGITHUB_TOKEN=ghp_..."}
            rows={2}
            spellCheck={false}
            aria-label="环境变量"
          />
        </Row>
      </Card>
    </Group>
  );
}
