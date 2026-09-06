import type { Dict } from "..";
import type zh from "../zh-CN/panels";

export default {
  /* ── Terminal ── */
  "panels.terminal.title": "Terminal",
  "panels.terminal.newTab": "New terminal",
  "panels.terminal.closeTab": "Close terminal",
  "panels.terminal.hide": "Hide terminal panel",
  "panels.terminal.badge.agent": "Agent",
  "panels.terminal.badge.exited": "Exited",
  "panels.terminal.agentTabTitle": "{title} (started by the agent)",
  "panels.terminal.share.on": "Sharing with the agent: it can read this terminal's output (click to stop)",
  "panels.terminal.share.off": "Share with the agent: let it read this terminal's output, but not stop it",
  "panels.terminal.share.mark": "Shared",
  "panels.terminal.share.failed": "Sharing failed",
  "panels.terminal.sendSelection": "Send the selection to the agent",
  "panels.terminal.sendSelection.hint": "Select some text in the terminal first, then send it from here",
  "panels.terminal.closeConfirm.title": "Close \u201c{title}\u201d?",
  "panels.terminal.closeConfirm.agentBody":
    "The agent started this service. Closing will terminate it immediately, and the agent may still depend on it.",
  "panels.terminal.closeConfirm.body":
    "A process is running in this terminal. Closing will terminate it immediately.",
  "panels.terminal.closeConfirm.confirm": "Close and terminate",
  "panels.terminal.reattachFailed": "Could not reattach to this terminal after reconnecting. It may have exited.",
  "panels.terminal.processExited": "[Process exited. The log stays here; click × on the tab to close.]",
  "panels.terminal.attachFailed":
    "Could not attach to this terminal. Its service may have exited; you can close this tab.",
  "panels.terminal.startFailed": "The terminal failed to start. Close this tab and open a new one to try again.",

  /* ── Browser panel ── */
  "panels.browser.back": "Back",
  "panels.browser.forward": "Forward",
  "panels.browser.starting": "Starting browser…",
  "panels.browser.address": "Address bar",
  "panels.browser.address.placeholder": "Enter a URL",
  "panels.browser.viewMode": "Viewport mode",
  "panels.browser.viewMode.fit": "Fit: render the page at the panel's width",
  "panels.browser.viewMode.web": "Web: render at {width}px desktop width, scaled to fit the panel",
  "panels.browser.pick": "Pick element",
  "panels.browser.pick.title": "Pick element: click an element in the panel to hand its selector to the agent",
  "panels.browser.pick.miss": "No element was hit. Click something on the page and try again.",
  "panels.browser.pick.failed": "Pick failed: {error}",
  "panels.browser.navFailed": "Could not open: {error}",
  "panels.browser.keyboard": "Page keyboard input",
  "panels.browser.empty.title": "Start browsing",
  "panels.browser.empty.hint": "Enter a URL to browse alongside the agent.",

  /* ── File preview ── */
  "panels.filePreview.openFailed": "Could not open",
  "panels.filePreview.showTree": "Show file tree",
  "panels.filePreview.hideTree": "Hide file tree",
  "panels.filePreview.pickOne": "Select a file on the right",
  "panels.filePreview.loadingViewer": "Loading viewer…",
  "panels.filePreview.reading": "Reading file…",
  "panels.filePreview.binary": "This is a binary file and cannot be previewed in the app.",

  /* ── File tree ── */
  "panels.fileTree.label": "Project files",
  "panels.fileTree.loading": "Loading…",
  "panels.fileTree.truncated": "{count} more items not shown",
  "panels.fileTree.truncated#one": "{count} more item not shown",
  "panels.fileTree.filter": "Filter files",
  "panels.fileTree.filter.placeholder": "Filter files…",
  "panels.fileTree.clear": "Clear",
  "panels.fileTree.openFromDisk": "Open from disk",
  "panels.fileTree.openFromDisk.title": "Open from disk… (⌘O)",
  "panels.fileTree.noMatch": "No matching files",
  "panels.fileTree.symlink": "Symbolic link",

  /* ── Git changes ── */
  "panels.git.base.title": "Comparison base. Only changes which branch to compare against; nothing is checked out.",
  "panels.git.currentBranch": "Current branch",
  "panels.git.recompare": "Compare again",
  "panels.git.comparing": "Comparing…",
  "panels.git.failed": "Comparison failed: {error}",
  "panels.git.stale": "The list below is from the last successful comparison.",
  "panels.git.notRepo": "This directory is not a git repository.",
  "panels.git.notRepo.hint": "Once you initialize a repository (git init), uncommitted changes will show up here.",
  "panels.git.noDiffAgainst": "No differences against {base}.",
  "panels.git.clean": "The workspace is clean; there are no uncommitted changes.",
  "panels.git.scope.title":
    "Differences between the workspace (including uncommitted work) and the selected branch. Switching branches only changes the comparison base; nothing is checked out. To see only what this session changed, use the changes bar above the composer.",
  "panels.git.baseLabel": "Comparison base: {base}",
  "panels.git.allUncommitted": "Showing all uncommitted git changes",
} satisfies Dict<typeof zh>;
