import { useState } from "react";

import {
  type AppConfig,
  type ConfigStatus,
  type PromptPreset,
  setConfig,
} from "../../bridge";
import { newPresetId, presetLabel, presetSummary } from "../../lib/prompts";
import { Card, Group, Row } from "./layout";
import { type AskConfirm, FormError, blurOnEnter } from "./shared";

/**
 * 提示词库。
 *
 * 存的只是一份素材清单 —— 内核从不读它。用户在会话设置里挑一条，
 * 那一刻正文被**复制**进会话；之后改这里不影响已经在跑的会话。
 * 这样"整理提示词库"永远是安全动作，不会牵动任何正在进行的对话。
 */
export function PromptsPane({
  status,
  onStatus,
  askConfirm,
  onSaved,
}: {
  status: ConfigStatus;
  onStatus: (s: ConfigStatus) => void;
  askConfirm: AskConfirm;
  onSaved: () => void;
}) {
  const cfg = status.config;
  const prompts = cfg.prompts ?? [];
  const [selId, setSelId] = useState(prompts[0]?.id ?? "");
  /** 刚新建的那条：编辑器聚焦到标题，省得用户自己找第一个待填字段。 */
  const [justAdded, setJustAdded] = useState("");
  const [error, setError] = useState("");

  const sel = prompts.find((p) => p.id === selId) ?? prompts[0];

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

  const patchSel = (patch: Partial<PromptPreset>) => {
    if (!sel) return;
    void commit({
      ...cfg,
      prompts: prompts.map((p) => (p.id === sel.id ? { ...p, ...patch } : p)),
    });
  };

  const add = () => {
    const id = newPresetId(prompts);
    void commit({ ...cfg, prompts: [...prompts, { id, title: "", body: "" }] }).then((ok) => {
      if (ok) {
        setSelId(id);
        setJustAdded(id);
      }
    });
  };

  const duplicate = () => {
    if (!sel) return;
    const id = newPresetId(prompts);
    const copy: PromptPreset = { id, title: `${presetLabel(sel)} 副本`, body: sel.body };
    void commit({ ...cfg, prompts: [...prompts, copy] }).then((ok) => {
      if (ok) {
        setSelId(id);
        setJustAdded(id);
      }
    });
  };

  const remove = () => {
    if (!sel) return;
    const target = sel;
    askConfirm({
      title: `删除提示词「${presetLabel(target)}」？`,
      body: "已经选用过它的会话不受影响 —— 那些会话里存的是当时复制过去的正文。",
      confirmLabel: "删除",
      action: () => {
        const rest = prompts.filter((p) => p.id !== target.id);
        void commit({ ...cfg, prompts: rest }).then((ok) => {
          if (ok) setSelId(rest[0]?.id ?? "");
        });
      },
    });
  };

  if (!sel) {
    return (
      <Group title="提示词">
        <div className="empty-state">
          <p className="empty-title">还没有收藏的提示词</p>
          <p className="hint">
            把常用的角色设定、输出格式要求、项目背景存在这里，开会话时挑一条填进
            「系统提示词」，不用每次重打一遍。
          </p>
          <div className="empty-actions">
            <button className="primary" onClick={add}>
              添加提示词
            </button>
          </div>
          {error ? <p className="form-error">{error}</p> : null}
        </div>
      </Group>
    );
  }

  return (
    <>
      <Group
        title="提示词"
        desc="会话设置的「系统提示词」里可以挑一条填进去。选中那一刻正文被复制过去，之后改这里不影响已有会话。"
        action={
          <div className="set-group-actions">
            <button className="btn-compact" onClick={add}>
              添加
            </button>
          </div>
        }
      >
        {/* 竖排列表：提示词的名字长度不可控，pill 铺排会被一条长标题撑爆。 */}
        <ul className="preset-list">
          {prompts.map((p) => (
            <li key={p.id}>
              <button
                className={p.id === sel.id ? "preset-row active" : "preset-row"}
                onClick={() => {
                  setSelId(p.id);
                  setJustAdded("");
                }}
                title={presetLabel(p)}
              >
                <span className="preset-row-name">{presetLabel(p)}</span>
                <span className="preset-row-meta">{presetSummary(p)}</span>
              </button>
            </li>
          ))}
        </ul>
      </Group>

      <PresetEditor
        key={sel.id}
        preset={sel}
        autoFocusTitle={sel.id === justAdded}
        onPatch={patchSel}
        onDuplicate={duplicate}
        onRemove={remove}
      />

      {error ? <FormError text={error} /> : null}
    </>
  );
}

function PresetEditor({
  preset,
  autoFocusTitle,
  onPatch,
  onDuplicate,
  onRemove,
}: {
  preset: PromptPreset;
  /** 刚新建时聚焦标题输入框。 */
  autoFocusTitle?: boolean;
  onPatch: (p: Partial<PromptPreset>) => void;
  onDuplicate: () => void;
  onRemove: () => void;
}) {
  const [title, setTitle] = useState(preset.title ?? "");
  const [body, setBody] = useState(preset.body);

  return (
    <Group title={presetLabel(preset)}>
      <Card>
        <Row title="名称" desc="只在挑选的时候显示。留空就用正文首行。">
          <input
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            onBlur={() => title.trim() !== (preset.title ?? "") && onPatch({ title: title.trim() })}
            onKeyDown={blurOnEnter}
            autoFocus={autoFocusTitle}
            placeholder="如：代码审查、翻译腔纠正"
            spellCheck={false}
            aria-label="名称"
          />
        </Row>
        <Row
          title="内容"
          desc="原样追加在内置提示词之后。适合放长期有效的指令（做什么、什么口吻、输出成什么样）；一次性的要求直接在对话里说更省事。"
          stack
        >
          <textarea
            className="preset-body-input"
            value={body}
            onChange={(e) => setBody(e.target.value)}
            onBlur={() => body.trim() !== preset.body && onPatch({ body: body.trim() })}
            rows={12}
            placeholder="给模型的指令…"
            spellCheck={false}
            aria-label="内容"
          />
        </Row>
      </Card>
      <div className="editor-foot">
        <span />
        <div className="editor-foot-actions">
          <button onClick={onDuplicate}>复制一份</button>
          <button className="btn-danger ghost-danger" onClick={onRemove}>
            删除提示词
          </button>
        </div>
      </div>
    </Group>
  );
}
