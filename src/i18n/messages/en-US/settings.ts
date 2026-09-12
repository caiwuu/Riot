import type { Dict } from "..";
import type zh from "../zh-CN/settings";

export default {
  "settings.title": "Settings",
  "settings.back": "Back to app",
  "settings.backTitle": "Back to app (Esc)",
  "settings.navLabel": "Settings sections",
  "settings.group.model": "Model",
  "settings.group.run": "Runtime",
  "settings.group.ext": "Extensions",
  "settings.group.app": "Application",
  "settings.tab.provider": "Providers",
  "settings.tab.provider.title": "Providers",
  "settings.tab.provider.desc": "Connect model services, store API keys, and set sampling parameters per model.",
  "settings.tab.web": "Web",
  "settings.tab.web.title": "Web access",
  "settings.tab.web.desc": "Whether the model can fetch pages and search, and which model condenses page text.",
  "settings.tab.prompts": "Prompts",
  "settings.tab.prompts.title": "Prompts",
  "settings.tab.prompts.desc": "Save frequently used system prompts and pick one when starting a session instead of retyping it.",
  "settings.tab.permission": "Permissions",
  "settings.tab.permission.title": "Permissions & runtime",
  "settings.tab.permission.desc": "Default permissions for new sessions, command isolation, approval timeout and per-turn limits.",
  "settings.tab.mcp": "MCP",
  "settings.tab.mcp.title": "MCP servers",
  "settings.tab.mcp.desc": "Connect external tool servers; once connected the model can call them directly.",
  "settings.tab.packs": "Packs",
  "settings.tab.packs.title": "Packs",
  "settings.tab.packs.desc": "Optional downloadable runtimes. Related tools and skills register automatically once installed.",
  "settings.tab.skills": "Skills",
  "settings.tab.skills.title": "Skills",
  "settings.tab.skills.desc": "Skills written as SKILL.md files, loaded by the model on demand.",
  "settings.tab.commands": "Commands",
  "settings.tab.commands.title": "Slash commands",
  "settings.tab.commands.desc": "Prompt templates invoked by typing / in the composer. The model never sees them and they take no context.",
  "settings.tab.hooks": "Hooks",
  "settings.tab.hooks.title": "Hooks",
  "settings.tab.hooks.desc": "Run scripts automatically at fixed checkpoints; they can block a tool call or a whole turn.",
  "settings.tab.general": "General",
  "settings.tab.general.title": "General",
  "settings.tab.general.desc": "Preferences that are not tied to any session, such as the interface language.",
  "settings.tab.remote": "Remote access",
  "settings.tab.remote.title": "Remote access",
  "settings.tab.remote.desc": "Use this same Riot from a phone or another computer's browser. Toggle, listen scope, login QR code and token.",
  "settings.tab.about": "About",
  "settings.tab.about.title": "About Riot",
  "settings.tab.about.desc": "Version, updates and where the config file lives.",
  "settings.configBroken.title": "Config file is corrupted",
  "settings.configBroken.hint": "Started with default settings. The original file is backed up at:",

  "settings.general.language": "Interface language",
  "settings.general.language.desc": "Takes effect immediately. Prompts sent to the model are not affected.",
  "settings.general.language.system": "Follow system",
  "settings.general.theme": "Appearance",
  "settings.general.theme.desc":
    "Takes effect immediately. With “Follow system”, it switches along with the system appearance.",
  "settings.general.theme.system": "Follow system",
  "settings.general.theme.light": "Light",
  "settings.general.theme.dark": "Dark",

  "settings.about.version": "Version",
  "settings.about.tagline": "A lightweight, powerful agent workbench",
  "settings.about.check": "Check for updates",
  "settings.about.checking": "Checking…",
  "settings.about.checkingStatus": "Checking…",
  "settings.about.download": "Download",
  "settings.about.newer": "New version {version} available",
  "settings.about.upToDate": "You're up to date",
  "settings.about.err.rateLimit": "GitHub is rate-limiting right now. Try again in a while.",
  "settings.about.err.noRelease": "No release has been published yet.",
  "settings.about.err.offline": "Can't reach the update service right now.",
  "settings.about.config": "Config file",
  "settings.about.config.desc": "API keys are not stored here; they live in {file} in the same directory.",

  "settings.common.name": "Name",
  "settings.common.testing": "Testing…",

  "settings.provider.newName": "Provider {n}",
  "settings.provider.remove.title": "Delete provider “{name}”?",
  "settings.provider.remove.last":
    "You'll need to add a provider again before sending messages. The API key is not deleted.",
  "settings.provider.remove.active": "The next provider becomes active automatically. The API key is not deleted.",
  "settings.provider.remove.other": "The API key is not deleted.",
  "settings.provider.empty.title": "No providers yet",
  "settings.provider.empty.hint": "Add a provider and enter its API key to start chatting.",
  "settings.provider.add": "Add provider",
  "settings.provider.roles": "Global model roles",
  "settings.provider.list.desc": "Pick the provider to edit. The dot marks the one the current conversation is using.",
  "settings.provider.inUse": "In use",
  "settings.provider.vision": "Vision fallback",
  "settings.provider.vision.desc":
    "Only applies to models without “Vision” checked: the model chosen here looks at the image and describes it in text, which is then handed to the main model. Models that accept images get the original directly. The description is lossy; don't rely on it for pixel-precise judgments.",
  "settings.provider.vision.none": "Don't convert (the screenshot tool reports it's unavailable)",
  "settings.provider.vision.noCandidates":
    "No model is marked as vision-capable yet. Check “Vision” on a vision model in the model list below first.",
  "settings.provider.vision.gone":
    "{model} is no longer available (the model was removed or its “Vision” flag was cleared); images are not being described right now.",
  "settings.provider.subagent": "Subagent budget model",
  "settings.provider.subagent.desc":
    "Read-only scouting subagents use this model. Browsing code and writing reports changes nothing, but all search results land in context, which tends to burn more tokens. Subagents that edit code always use the main model.",
  "settings.provider.subagent.main": "Same as main model",
  "settings.provider.subagent.gone":
    "{model} is no longer available (the model was removed); scouting currently uses the main model.",

  "settings.provider.editor.model.remove.title": "Remove model “{model}”?",
  "settings.provider.editor.model.remove.active":
    "It's currently in use. After removing it you'll need to pick another model before sending messages.",
  "settings.provider.editor.model.remove.other": "Only removes it from the list; you can add it back anytime.",
  "settings.provider.editor.test.noModel": "Add a model first (enter one manually or fetch from the API), then test.",
  "settings.provider.editor.connection": "Connection",
  "settings.provider.editor.name.desc": "Display only; call it whatever you like.",
  "settings.provider.editor.protocol": "Protocol",
  "settings.provider.editor.protocol.desc":
    "Determines the request format and auth header. The wrong one gets rejected by the provider.",
  "settings.provider.editor.protocol.openai": "OpenAI-compatible",
  "settings.provider.editor.baseUrl": "API host",
  "settings.provider.editor.apiPath": "API path",
  "settings.provider.editor.apiPath.desc":
    "Leave empty to infer from the host. Fill it in when the endpoint is somewhere unusual (e.g. Zhipu's {example}).",
  "settings.provider.editor.urlPreview": "Effective request URL",
  "settings.provider.editor.headers": "Request headers",
  "settings.provider.editor.headers.desc":
    "One Name=Value per line. Use {session} for the current conversation ID (stable for the whole chat). Auth headers cannot be overridden here.",
  "settings.provider.editor.headers.placeholder": "e.g.\nx-opencode-session=${session_id}",
  "settings.provider.editor.headers.format": "Headers must be written as Name=Value: “{line}”",
  "settings.provider.editor.key": "Key",
  "settings.provider.editor.key.saved": "Saved.",
  "settings.provider.editor.key.env": "Using environment variable {env}.",
  "settings.provider.editor.key.savedOverride": "Saved. Paste a new one to replace it.",
  "settings.provider.editor.key.missing": "Not set yet; messages can't be sent until it is.",
  "settings.provider.editor.key.placeholder": "Paste the API key for {name}",
  "settings.provider.editor.models": "Models",
  "settings.provider.editor.addModel": "Add model…",
  "settings.provider.editor.fetchNeedsKey": "Save an API key above first to fetch the model list",
  "settings.provider.editor.fetching": "Fetching…",
  "settings.provider.editor.fetch": "Fetch from API",
  "settings.provider.editor.models.empty":
    "No models yet. Use “Add model” in the top right to enter one manually, or fetch from the API.",
  "settings.provider.editor.testModel": "Model used for the connection test",
  "settings.provider.editor.testWith": "Test the connection with this model instead",
  "settings.provider.editor.vision.aria": "Accepts images",
  "settings.provider.editor.vision.title": "This model accepts images",
  "settings.provider.editor.editModel": "Edit model",
  "settings.provider.editor.removeFromList": "Remove from list",
  "settings.provider.editor.clickRemove": "Click to remove",
  "settings.provider.editor.clickAdd": "Click to add",
  "settings.provider.editor.fetched.empty": "This provider returned no models.",
  "settings.provider.editor.sampling": "Sampling",
  "settings.provider.editor.sampling.desc":
    "Defaults for this provider. Fields marked “Model default” are not sent at all and left to the model. Fields a model doesn't set itself use these values; change a single model in its edit dialog, and override per session in the conversation.",
  "settings.provider.editor.test.hint": "Send a minimal request to verify the configuration.",
  "settings.provider.editor.test.hintModel": "Send a minimal request with {model} to verify the configuration.",
  "settings.provider.editor.testNeedsKey": "Save an API key above first to test the connection",
  "settings.provider.editor.test": "Test connection",
  "settings.provider.editor.test.ok": "Connected: {detail}",

  "settings.web.access": "Web access",
  "settings.web.fetch": "Fetch pages",
  "settings.web.fetch.desc":
    "Let the model open links and read page text (WebFetch). The first visit to each domain asks for approval; private network addresses are always refused.",
  "settings.web.search": "Web search",
  "settings.web.search.desc": "Let the model run searches on its own and read the results (WebSearch).",
  "settings.web.searxng": "Custom SearXNG",
  "settings.web.searxng.desc":
    "Leave empty to use the built-in search. A self-hosted instance needs {limiter}, and {formats} must include {json}.",
  "settings.web.searxng.placeholder": "Leave empty for built-in search",
  "settings.web.test.needsSearch": "Turn on web search above first",
  "settings.web.test.custom": "Sends a real query",
  "settings.web.test.builtin": "Tests the built-in search",
  "settings.web.test": "Test",
  "settings.web.test.ok": "Search works: {detail}",
  "settings.web.distill": "Page condensing",
  "settings.web.distill.model": "Helper model",
  "settings.web.distill.model.desc":
    "Use a cheap model to condense pages into summaries and save context. Without one, page text is simply truncated.",
  "settings.web.distill.none": "Don't condense (return truncated text)",
  "settings.web.distill.gone": "{model} no longer exists; pages are not being condensed right now.",

  "settings.prompts.copyName": "{name} (copy)",
  "settings.prompts.remove.title": "Delete prompt “{name}”?",
  "settings.prompts.remove.body":
    "Sessions that already use it are unaffected; they keep the copy made when it was picked.",
  "settings.prompts.empty.title": "No saved prompts yet",
  "settings.prompts.empty.hint":
    "Keep frequently used personas, output format requirements and project background here. Pick one for the system prompt when starting a session instead of retyping it.",
  "settings.prompts.add": "Add prompt",
  "settings.prompts.desc":
    "Pick one in the session settings' system prompt field. The text is copied at that moment, so later edits here don't affect existing sessions.",
  "settings.prompts.name.desc": "Shown only when picking. Leave empty to use the first line of the content.",
  "settings.prompts.name.placeholder": "e.g. Code review, Fix translationese",
  "settings.prompts.body": "Content",
  "settings.prompts.body.desc":
    "Appended verbatim after the built-in prompt. Best for standing instructions (what to do, what tone, what the output should look like); one-off requests are easier to just say in the conversation.",
  "settings.prompts.body.placeholder": "Instructions for the model…",
  "settings.prompts.duplicate": "Duplicate",
  "settings.prompts.remove": "Delete prompt",

  "settings.permission.mode.default": "Ask every time",
  "settings.permission.mode.default.desc": "Asks before writing files or running commands.",
  "settings.permission.mode.acceptEdits": "Allow edits",
  "settings.permission.mode.acceptEdits.desc": "File edits go through; commands still ask.",
  "settings.permission.mode.plan": "Plan",
  "settings.permission.mode.plan.desc": "Read-only investigation that produces a plan; acts only after approval.",
  "settings.permission.mode.auto": "Auto-assess",
  "settings.permission.mode.auto.desc":
    "A small model screens each action first; clearly safe ones aren't asked about. Safety checks and your rules still apply. Requires a subagent budget model.",
  "settings.permission.mode.bypassPermissions": "Allow all",
  "settings.permission.mode.bypassPermissions.desc":
    "Routine actions aren't asked about; dangerous ones are still blocked.",
  "settings.permission.mode.unattended": "Unattended",
  "settings.permission.mode.unattended.desc":
    "Everything goes through, including dangerous actions. Disposable environments only.",
  "settings.permission.highRisk": "High risk",
  "settings.permission.defaultMode": "Default permissions for new sessions",
  "settings.permission.defaultMode.desc":
    "Only affects sessions created from now on. The current session's permissions are switched from the dropdown next to the session title in the top bar.",
  "settings.permission.defaultMode.aria": "Default mode for new sessions",
  "settings.permission.unattendedConfirm.title": "Set the default mode to Unattended?",
  "settings.permission.unattendedConfirm.body":
    "Every new session will skip all permission checks, including dangerous actions.",

  "settings.permission.sandbox": "Command isolation",
  "settings.permission.sandbox.desc":
    "The operating system limits what commands can modify. While on, commands that match no rule and aren't read-only run without asking; the kernel holds the boundary. Works out of the box on macOS; Windows needs a one-time install.",
  "settings.permission.sandbox.workspaceWrite": "Isolated (recommended)",
  "settings.permission.sandbox.workspaceWrite.desc":
    "Can only modify the workspace, temp directories, and build caches; on macOS, Desktop / Downloads too. Reading and network are unrestricted.",
  "settings.permission.sandbox.workspaceWriteNoNet": "Isolated, no network",
  "settings.permission.sandbox.workspaceWriteNoNet.desc":
    "Also cuts the command's network access. npm and cargo will fail to fetch dependencies.",
  "settings.permission.sandbox.off": "Not isolated",
  "settings.permission.sandbox.off.desc": "Commands can modify any file; only rule checks stand in the way.",
  "settings.permission.sandbox.probeFailed":
    "Couldn't determine whether isolation is active ({error}). The selection below may not reflect reality.",
  "settings.permission.sandbox.installedOff":
    "System-level isolation is installed but not enabled (“Not isolated” is selected above). Uninstall it if you no longer need it:",
  "settings.permission.sandbox.installed": "System-level isolation is installed.",
  "settings.permission.sandbox.waitingUac": "Waiting for permission prompt…",
  "settings.permission.sandbox.uninstall": "Uninstall (requires administrator)",
  "settings.permission.sandbox.install": "Install (requires administrator)",
  "settings.permission.sandbox.unsupported":
    "This platform has no system-level isolation yet; selecting it has no effect and every command still asks.",
  "settings.permission.sandbox.notInstalled":
    "Not installed, so nothing is isolated right now: commands run directly, with only rule checks and per-command prompts in the way.",
  "settings.permission.sandbox.broken": "Installed but not working; nothing is isolated right now: {error}",
  "settings.permission.sandbox.noNetIsolation":
    "This platform can't cut network access, so this level falls back to no isolation. Choose “Isolated (recommended)” instead.",
  "settings.permission.sandbox.installConfirm.title": "Install command isolation now?",
  "settings.permission.sandbox.installConfirm.body":
    "Windows will ask for permission (UAC) twice: first to create a dedicated low-privilege account, then to lift its built-in network restriction (without that, the sandbox has no network at all). This only needs to be done once.",
  "settings.permission.sandbox.installConfirm.confirm": "Install",
  "settings.permission.sandbox.uninstallConfirm.title": "Uninstall command isolation?",
  "settings.permission.sandbox.uninstallConfirm.body":
    "Windows will ask for permission (UAC) once to delete the sandbox account, its credentials and leftover grants. Commands will no longer be isolated at the system level; you can reinstall anytime.",
  "settings.permission.sandbox.uninstallConfirm.confirm": "Uninstall",
  "settings.permission.sandbox.offConfirm.title": "Turn off command isolation?",
  "settings.permission.sandbox.offConfirm.body":
    "Commands will be able to modify any file outside the workspace, with only rule checks in the way. Rules can't read the code inside “python -c \"...\"”.",
  "settings.permission.sandbox.offConfirm.confirm": "Turn off",
  "settings.permission.allowRead": "Extra readable directories in the sandbox",
  "settings.permission.allowRead.desc":
    "One absolute path per line. Tools installed under your user directory (nvm, conda, pip --user…) can't be opened inside the sandbox by default; add the ones you need. Larger directories make the first activation of a session slower.",
  "settings.permission.allowRead.placeholder": "e.g.\nC:\\Users\\you\\.cargo\nC:\\Users\\you\\.rustup",

  "settings.permission.limits": "Runtime limits",
  "settings.permission.clamp.max": "Adjusted to the maximum, {value}",
  "settings.permission.clamp.min": "Adjusted to the minimum, {value}",
  "settings.permission.timeout": "Approval timeout",
  "settings.permission.timeout.desc":
    "How long a prompt waits for an answer before giving up; a timeout counts as a refusal. Range {min}–{max} seconds.",
  "settings.permission.timeout.aria": "Approval timeout (seconds)",
  "settings.permission.unit.seconds": "sec",
  "settings.permission.turns": "Per-turn step limit",
  "settings.permission.turns.desc":
    "How many steps the model may take on its own within one message. At the limit it stops and waits for you; it's not an error. Multi-step tasks like browser automation or pentesting fill it up easily, so raise it if needed. Range {min}–{max} steps.",
  "settings.permission.unit.steps": "steps",
  "settings.permission.compactAt": "Default compaction threshold",
  "settings.permission.compactAt.desc":
    "When the estimated session history exceeds this many tokens, it's automatically summarized and compacted. Only applies to models {noWindow}; models with a window use that instead. Range {min}–{max}.",
  "settings.permission.compactAt.noWindow": "without a context window set",
  "settings.permission.compactAt.aria": "Default compaction threshold (tokens)",

  "settings.permission.memory": "Memory",
  "settings.permission.recall": "Session recall",
  "settings.permission.recall.desc":
    "Lets the model look through this project's other sessions: answer “how did we end up solving that last time” and link to that session. Excerpts are stored per project in Riot's own data directory, never in the project. When off, the prompt no longer mentions it; each session still maintains its own excerpts, which the model uses to recover summarized-away text after compaction. Excerpts are deleted along with the session.",
  "settings.permission.rules": "Session rules",
  "settings.permission.rules.hint":
    "Rules remembered by clicking “Always allow” (e.g. {example}) only last for the current session and are gone once it's closed.",
} satisfies Dict<typeof zh>;
