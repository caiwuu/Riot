import type { Dict } from "..";
import type zh from "../zh-CN/composer";

export default {
  "composer.placeholder": "Describe a task, or ask anything",
  "composer.placeholder.plan": "Ask for changes to the plan, or press Build to start",
  "composer.build": "Build",
  "composer.build.title": "Carry out the plan (returns to the permission level used before planning)",
  "composer.build.more": "More ways to build",
  "composer.build.parallel": "Build in parallel",
  "composer.build.parallelHint": "Multi-task mode; background subagents split the work",
  "composer.build.message": "Build the plan",
  "composer.placeholder.busy":
    "It's working… sending now queues the message; it goes out once the current task finishes",
  "composer.drop.hint": "Release to add to the composer",
  "composer.closeEsc": "Close (Esc)",
  "composer.undo": "Undo",

  "composer.banner.workspaceMissing":
    "The project folder is no longer on disk. Click here to remove it or pick another folder.",
  "composer.banner.noProvider": "No provider configured yet. Click here to add one",
  "composer.banner.noKey": "{provider} has no API key yet. Click here to set it up",
  "composer.banner.noModel": "{provider} has no model selected yet. Click here to set it up",
  "composer.banner.currentProvider": "The current provider",
  "composer.banner.dismiss": "Click to dismiss",

  "composer.queue.count": "{count} queued",
  "composer.queue.imageOnly": "(images only)",
  "composer.queue.images": "{count} images",
  "composer.queue.images#one": "{count} image",
  "composer.queue.edit.title": "Edit (put back into the composer)",
  "composer.queue.sendNow": "Send now",
  "composer.queue.sendNow.title": "Send now (stop the current turn and handle this one first)",
  "composer.queue.imageLabel": "Queued image",
  "composer.withdrawn.imageLabel": "Withdrawn image",

  "composer.slash.menuLabel": "Slash commands",
  "composer.slash.group.skills": "Skills",
  "composer.slash.group.commands": "Commands",
  "composer.slash.expandFailed": "Failed to expand {name}: the command may have just been deleted",
  "composer.mention.menuLabel": "File references",

  "composer.attach.title": "Attach images or files",
  "composer.attach.title.web": "Attach images (use @ to reference files)",
  "composer.attach.pastedImage": "Pasted image",
  "composer.attach.view": "View {name}",
  "composer.attach.thisFile": "This file",
  "composer.attach.tooMany": "A message can hold at most {max} images; {extra} extra were ignored.",
  "composer.attach.notImage":
    "{name} is not an image and the system did not provide its path. Use the \"+\" button at the bottom left, or type @ in the composer to find it.",
  "composer.attach.noDiskFile":
    "The dropped item has no file on disk (most likely an image dragged straight from a web page). Copy it, then come back here and press {key}.",

  "composer.provider.switch": "Switch provider",
  "composer.provider.pick": "Choose a provider",
  "composer.provider.noKey": "No key",
  "composer.model.switch": "Switch model",
  "composer.model.pick": "Choose a model",
  "composer.model.empty": "This provider has no models yet",
  "composer.model.vision": "Accepts images",
  "composer.contextWindow": "Context window",
  "composer.contextWindow.followSettings": "Follow settings",

  "composer.handoff": "Move to background",
  "composer.handoff.pending": "Moving to background…",
  "composer.handoff.title":
    "Hand the current task to a background sub-agent and free up the conversation",
  "composer.handoff.pendingTitle":
    "The model has been told. It will fork the task to the background once it finishes the current step",
  "composer.stop.title": "Stop (Esc)",
  "composer.send": "Send",
  "composer.send.queue": "Queue",
  "composer.send.queueTitle": "Queue (sent automatically once the current task finishes)",
  "composer.send.pickModel": "Choose a model first",

  "composer.mode.group": "Work mode",
  "composer.mode.title": "Work mode: {mode}",
  "composer.mode.agent": "Agent",
  "composer.mode.agent.hint": "Acts as it goes, executing directly under the current permission mode.",
  "composer.mode.plan": "Plan",
  "composer.mode.plan.hint": "Read-only reconnaissance that produces a plan; acts only after approval.",
  "composer.mode.multitask": "Multitask",
  "composer.mode.multitask.hint":
    "The main agent only coordinates; the real work goes to background sub-agents. It ends its turn after delegating and notifies you when done. Good for tasks that take minutes or more, so you can keep chatting while you wait.",

  "composer.perm.group": "Permissions",
  "composer.perm.title": "Permissions: {mode}",
  "composer.perm.titleWarn": "Permissions: {mode} ({warn})",
  "composer.perm.default": "Ask every time",
  "composer.perm.acceptEdits": "Allow edits",
  "composer.perm.plan": "Plan",
  "composer.perm.auto": "Auto-assess risk",
  "composer.perm.bypassPermissions": "Allow all",
  "composer.perm.bypassPermissions.warn": "At your own risk",
  "composer.perm.unattended": "Unattended",
  "composer.perm.unattended.warn": "Includes dangerous actions",
  "composer.perm.unattended.confirm.title": "Switch to unattended?",
  "composer.perm.unattended.confirm.body":
    "This session will no longer show any permission prompts, including for dangerous actions.",
  "composer.perm.unattended.confirm.ok": "Switch",

  "composer.sampling.title": "Sampling",
  "composer.sampling.modelDefault": "Model default",
  "composer.sampling.custom": "Custom",
  "composer.sampling.inherit": "Inherits {value}",
  "composer.sampling.clear": "Clear {field}, back to {value}",
  "composer.sampling.state.offAria":
    "{field} is currently model default. Click to inherit {value} again",
  "composer.sampling.state.offTitle": "Inherit again ({value})",
  "composer.sampling.state.inheritAria":
    "{field} currently inherits {value}. Click to use the model default",
  "composer.sampling.state.inheritTitle":
    "Use the model default: this field is not sent to the provider; the model decides",
  "composer.sampling.temperature.hint": "0–2. Higher is more random.",
  "composer.sampling.topP.hint": "0–1. Nucleus sampling. Usually not tuned together with temperature.",
  "composer.sampling.topK.hint": "Sent only over the Anthropic protocol.",
  "composer.sampling.maxOutputTokens.hint": "Output cap for a single reply.",

  "composer.thinking.title": "Thinking effort",
  "composer.thinking.hint":
    "How deeply a reasoning model thinks per request (reasoning_effort / thinking). Adaptive = medium for new instructions, low for tool follow-ups, saving time and cost. \"Default\" sends no parameter at all. Levels and Off require endpoint support (DeepSeek, GLM, OpenAI reasoning models); unsupported endpoints reject the request, in which case switch back to \"Default\". Takes effect next turn.",
  "composer.thinking.default.hint": "No thinking parameter; endpoint default",
  "composer.thinking.adaptive": "Adaptive",
  "composer.thinking.adaptive.hint": "Thinks hard on the first turn, less on tool follow-ups",
  "composer.thinking.low": "Low",
  "composer.thinking.medium": "Medium",
  "composer.thinking.high": "High",
  "composer.thinking.disabled": "Thinking off",
  "composer.thinking.disabled.hint": "Not supported by some endpoints",

  "composer.sessionSettings.title": "Session settings",
  "composer.sessionSettings.newSession": "New session",
  "composer.sessionSettings.sampling.hint":
    "Grey values are inherited. Only fields you drag or edit are written as overrides; click the text under a slider to switch to \"Model default\" — that field is then not sent at all and the model decides.",
  "composer.sessionSettings.sampling.reset": "Reset all to inherited",
  "composer.sessionSettings.sampling.resetDone": "Reset",
  "composer.sessionSettings.venv": "Python virtual environment",
  "composer.sessionSettings.venv.hint":
    "Sets VIRTUAL_ENV and puts its bin first on PATH so python / pip resolve to this environment. Clear to restore the system default. The system picker hides dot-folders like .venv by default (⌘⇧. toggles them); you can also type the path directly.",
  "composer.sessionSettings.venv.placeholder": "venv root folder",
  "composer.sessionSettings.venv.pick": "Choose…",
  "composer.sessionSettings.venv.found": "Found {name}, use it",
  "composer.sessionSettings.prompt": "System prompt",
  "composer.sessionSettings.prompt.hint":
    "Appended after the built-in prompt, not replacing it. Leave empty to use only the built-in prompt. Picking a saved prompt fills the box below, and you can still edit it — edits belong to this session only and never touch the saved copy.",
  "composer.sessionSettings.prompt.save": "Save as prompt",
  "composer.sessionSettings.prompt.saved": "Saved",
  "composer.sessionSettings.prompt.pickTitle": "Pick one of your saved prompts",
  "composer.sessionSettings.prompt.none": "None",
  "composer.sessionSettings.prompt.none.hint": "Built-in prompt only",
  "composer.sessionSettings.prompt.custom": "Custom",
  "composer.sessionSettings.prompt.custom.hint": "Hand-written, not in the library",
  "composer.sessionSettings.prompt.placeholder": "Extra instructions for this session",
  "composer.sessionSettings.prompt.replaced": "Your previous text was replaced",

  "composer.modelDialog.add": "Add model",
  "composer.modelDialog.edit": "Edit model",
  "composer.modelDialog.id": "Model ID",
  "composer.modelDialog.id.placeholder": "Model name sent to the provider, e.g. glm-4.6v",
  "composer.modelDialog.duplicate": "This provider already has a model with that ID.",
  "composer.modelDialog.name": "Display name",
  "composer.modelDialog.name.placeholder": "Leave empty to show the model ID",
  "composer.modelDialog.capabilities": "Capabilities",
  "composer.modelDialog.vision": "Vision (accepts images)",
  "composer.modelDialog.vision.hint":
    "When off, screenshots and attached images are first converted to text by the \"vision fallback model\". Turning it on for a model without vision makes the provider reject images.",
  "composer.modelDialog.window.hint":
    "The window size from the model's documentation. When set, compaction timing is computed for this model; leave empty to use the global threshold from Settings.",
  "composer.modelDialog.window.placeholder": "e.g. 128000",
  "composer.modelDialog.window.noteEmpty": "Empty = follow the global compaction threshold in Settings.",
  "composer.modelDialog.window.note":
    "Compacts automatically at about {tokens} of history (window minus the reserve for the reply and summary).",
  "composer.modelDialog.sampling.hint":
    "Grey values come from the provider level. Only edited fields are written as overrides; to stop sending a field entirely (reasoning models often reject temperature), click the text under its slider to switch to \"Model default\".",
  "composer.modelDialog.test": "Test model",
  "composer.modelDialog.testing": "Testing…",
  "composer.modelDialog.test.hint": "\"Test\" sends a real minimal request with this model.",

  "composer.prompts.empty": "Empty prompt",
  "composer.prompts.noBody": "No content yet",

  "composer.chip.preview": "Preview {path}",
  "composer.textarea.resize": "Drag to resize height",
} satisfies Dict<typeof zh>;
