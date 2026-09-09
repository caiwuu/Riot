import type { Dict } from "..";
import type zh from "../zh-CN/kernel";

export default {
  // ── RPC & sessions ─────────────────────────────────────────
  "kernel.rpc.badRequest": "The kernel received a request it could not parse.",
  "kernel.rpc.unimplemented": "Method {method} is not implemented in the kernel yet.",
  "kernel.session.notFound": "Session {id} does not exist.",
  "kernel.turn.busy": "A turn is still running. Wait for it to finish first.",
  "kernel.turn.panicked": "Internal error; this turn did not complete. Try again, or rephrase.",
  "kernel.turn.toolBatchLost": "The tool batch returned no results; this turn was aborted.",

  // ── Context editing / regenerate ───────────────────────────
  "kernel.history.emptyText": "The text cannot be empty. To remove this message, delete it instead.",
  "kernel.history.notInContext": "This message is no longer in the current context.",
  "kernel.history.compacted":
    "This message has been compacted into the summary; the model only sees the summary, so editing it has no effect.",
  "kernel.history.noPromptBefore": "No user message precedes this reply, so it cannot be regenerated.",
  "kernel.history.resendNotPrompt": "Only user messages can be resent. To change a reply, use Edit.",
  "kernel.history.noText": "This message has no editable text.",
  "kernel.history.systemNoText": "This is a system notice; it has no editable text.",
  "kernel.history.systemNoDelete": "This is a system notice; it cannot be deleted.",

  // ── Compact ────────────────────────────────────────────────
  "kernel.compact.empty": "There is no conversation to compact yet.",
  "kernel.compact.failed": "Compaction failed; the history is unchanged. Try again later.",

  // ── Provider (model calls) ─────────────────────────────────
  "kernel.provider.missingKey": "API key is missing. Add one for this provider in Settings.",
  "kernel.provider.httpClient": "Failed to initialise the HTTP client.",
  "kernel.provider.auth": "The provider rejected the API key. Check it in Settings.",
  "kernel.provider.rateLimited": "The provider is rate limiting. Wait a moment and try again.",
  "kernel.provider.overloaded": "The provider is overloaded and still failing after retries. Try again later.",
  "kernel.provider.backgroundOverloaded": "The provider is overloaded; this background request was skipped.",
  "kernel.provider.retriesExhausted": "Still failing after several retries.",
  "kernel.provider.quota": "The provider account is out of credit.",
  "kernel.provider.modelNotFound": "The provider does not know this model. Check the model name in Settings.",
  "kernel.provider.refused": "The provider refused the request (HTTP {status}).",
  "kernel.provider.htmlResponse":
    "The provider returned a web page instead of an API response (HTTP {status}). It was probably blocked by a gateway or firewall, or the API URL is wrong.",
  "kernel.provider.refusedInStream": "The provider reported an error mid-response.",
  "kernel.provider.transport": "Cannot reach the provider. Check your network, proxy or base URL.",
  "kernel.provider.unreachable": "Still cannot reach the provider after several attempts. Check your network, proxy or base URL.",
  "kernel.provider.timeout": "The request timed out; the network or provider did not respond in time. Retrying usually helps.",
  "kernel.provider.streamBroken": "The provider's response was cut off.",
  "kernel.provider.idle": "The provider sent nothing for {secs}s, so the request was ended. This is usually a proxy or gateway issue.",
  "kernel.provider.contextOverflow": "Context overflow: {used} tokens used, limit {limit}.",
  "kernel.provider.outputLimit": "Output token limit reached.",
  "kernel.provider.outputLimitExhausted": "Output tokens ran out {count} times in a row; the task needs more output than the model can produce.",
  "kernel.provider.mediaTooLarge": "Attachment too large ({bytes} bytes).",

  // ── Hooks ──────────────────────────────────────────────────
  "kernel.hook.promptBlocked": "Message blocked by a UserPromptSubmit hook: {reason}",
  "kernel.hook.stopBlocked": "A stop hook asked to continue: {reason}",
  "kernel.hook.badShape": "hooks.json has an unexpected structure.",

  // ── MCP ────────────────────────────────────────────────────
  "kernel.mcp.notRunning": "No MCP server named \u201c{id}\u201d is running. Enable it in Settings first.",
  "kernel.mcp.commandNotFound":
    "Failed to start: command \u201c{command}\u201d not found. Apps launched from Finder or the Dock do not get the terminal's PATH; use the absolute path from `which {command}`, or make sure npx / uvx / node is installed.",
  "kernel.mcp.spawnFailed": "Failed to start. Check the command path and arguments.",
  "kernel.mcp.disconnected": "The process exited or the connection dropped. Click Reconnect to try again.",
  "kernel.mcp.stopped": "Stopped.",
  "kernel.mcp.closed": "Connection closed (the server process may have exited).",
  "kernel.mcp.timeout": "{method} got no response within {secs}s.",
  "kernel.mcp.serverError": "The server reported an error ({code}).",
  "kernel.mcp.cancelled": "Cancelled.",
  "kernel.mcp.badResponse": "The server's response had an unexpected shape.",

  // ── Schedules ──────────────────────────────────────────────
  "kernel.schedule.unavailable": "No schedule service is available in this environment, so scheduled tasks cannot be created.",
  "kernel.schedule.hostUnavailable": "Cannot reach the host's scheduler.",
  "kernel.schedule.hostRejected": "The host did not accept this schedule operation.",

  // ── Subagents (Task) ───────────────────────────────────────
  "kernel.task.activity.started": "Starting",
  "kernel.task.activity.tool": "→ {name}",
  "kernel.task.activity.said": "{text}",
  "kernel.task.activity.completed": "Completed",
  "kernel.task.activity.failed": "Failed",
  "kernel.task.activity.cancelled": "Stopped",
  "kernel.task.activity.interrupted": "Interrupted by a Riot restart",
  "kernel.task.started": "[{kind}·{model}] {title} started",
  "kernel.task.startedBackground": "[{kind}·{model}] {title} started in the background",
  "kernel.task.launched": "{title} started in the background ({id})",
  "kernel.task.completed": "{title} completed · {model} · {tokens} tokens · {count} tool calls",

  // ── Skills & commands (Settings) ───────────────────────────
  "kernel.skill.noFrontmatter": "Missing frontmatter: the file must start with --- and include at least a description line.",
  "kernel.skill.unterminatedFrontmatter": "The frontmatter has no closing ---.",
  "kernel.skill.noDescription": "Missing description \u2014 it is the only thing the model uses to decide whether to load the skill.",
  "kernel.skill.emptyBody": "The body is empty: write how this skill works after the frontmatter.",
  "kernel.slash.compact": "Compact the conversation into a summary to free up context",

  // ── Config (Settings) ──────────────────────────────────────
  "kernel.config.unreadable": "The file could not be read.",
  "kernel.config.badJson": "Not valid JSON.",
  "kernel.config.encode": "Failed to serialise the config.",
  "kernel.config.mcpImportShape": 'Unexpected shape. Expected {"mcpServers": {"name": {"command": …}}}',
  "kernel.config.mcpImportEmpty": "It contains no servers.",
  "kernel.config.mcpImportUnnamed": 'Unexpected shape. Each server needs a name: {"mcpServers": {"name": {"command": …}}}',
  "kernel.config.mcpServerBad": "Could not parse \u201c{name}\u201d.",
  "kernel.config.mcpRemoteUnsupported": "\u201c{name}\u201d is an http/sse remote server; Riot currently only supports stdio (command + args).",
  "kernel.config.mcpMissingCommand": "\u201c{name}\u201d has no command.",
  "kernel.config.mcpIdClash": "\u201c{name}\u201d collides with another server's id after sanitising ({id}). Rename one of them.",
  "kernel.config.mcpIdEmpty": "An MCP server id cannot be empty.",
  "kernel.config.mcpIdInvalid": "MCP server id \u201c{id}\u201d may only contain letters, digits, - and _ (it becomes part of tool names).",
  "kernel.config.mcpIdDuplicate": "MCP server id \u201c{id}\u201d is duplicated.",
  "kernel.config.promptIdEmpty": "A prompt id cannot be empty.",
  "kernel.config.promptIdDuplicate": "Prompt id \u201c{id}\u201d is duplicated.",
  "kernel.config.noProvider": "No provider is configured yet. Add one in Settings.",
  "kernel.config.providerNotFound": "Provider \u201c{id}\u201d not found.",
  "kernel.config.noModelSelected":
    "\u201c{name}\u201d has no model selected. Add and select one in Settings, or pick one from the model menu in the composer.",
} satisfies Dict<typeof zh>;
