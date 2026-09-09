import type { Dict } from "..";
import type zh from "../zh-CN/kernel";

export default {
  // ── RPC 및 세션 ──────────────────────────────────────────────
  "kernel.rpc.badRequest": "커널이 해석할 수 없는 요청을 받았습니다.",
  "kernel.rpc.unimplemented": "메서드 {method}(은)는 아직 커널에 구현되지 않았습니다.",
  "kernel.session.notFound": "세션 {id}(이)가 존재하지 않습니다.",
  "kernel.turn.busy": "턴이 진행 중입니다. 끝난 뒤 다시 시도하세요.",
  "kernel.turn.panicked": "내부 오류로 이 턴이 완료되지 않았습니다. 다시 시도하거나 다른 표현으로 말해 보세요.",
  "kernel.turn.toolBatchLost": "도구 배치가 결과를 반환하지 않아 이 턴을 중단했습니다.",

  // ── 컨텍스트 편집 / 다시 생성 ──────────────────────────────────
  "kernel.history.emptyText": "내용은 비워 둘 수 없습니다. 이 메시지를 없애려면 삭제를 사용하세요.",
  "kernel.history.notInContext": "이 메시지는 더 이상 현재 컨텍스트에 없습니다.",
  "kernel.history.compacted": "이 메시지는 요약으로 압축되어 모델은 요약만 봅니다 — 수정해도 컨텍스트에 영향이 없습니다.",
  "kernel.history.noPromptBefore": "이 답변 앞의 사용자 메시지를 찾을 수 없어 다시 생성할 수 없습니다.",
  "kernel.history.resendNotPrompt": "다시 보내기는 사용자 메시지에서만 할 수 있습니다. 답변을 고치려면 「편집」을 사용하세요.",
  "kernel.history.noText": "이 메시지에는 편집할 수 있는 텍스트가 없습니다.",
  "kernel.history.systemNoText": "이 메시지는 시스템 프롬프트라 편집할 수 있는 텍스트가 없습니다.",
  "kernel.history.systemNoDelete": "이 메시지는 시스템 프롬프트라 삭제할 수 없습니다.",

  // ── 압축 ────────────────────────────────────────────────────
  "kernel.compact.empty": "아직 대화 내용이 없어 압축할 것이 없습니다.",
  "kernel.compact.failed": "압축에 실패했습니다. 기록은 그대로 유지됩니다. 나중에 다시 시도하세요.",

  // ── 제공자 (모델 호출) ────────────────────────────────────────
  "kernel.provider.missingKey": "API key가 없습니다. 설정에서 이 제공자에 입력하세요.",
  "kernel.provider.httpClient": "HTTP 클라이언트를 초기화할 수 없습니다.",
  "kernel.provider.auth": "제공자가 API key를 거부했습니다. 설정에서 확인하세요.",
  "kernel.provider.rateLimited": "제공자가 요청을 제한하고 있습니다. 잠시 후 다시 보내세요.",
  "kernel.provider.overloaded": "제공자가 과부하 상태라 여러 번 다시 시도했지만 실패했습니다. 잠시 후 다시 시도하세요.",
  "kernel.provider.backgroundOverloaded": "제공자가 과부하 상태라 이 백그라운드 요청은 건너뛰었습니다.",
  "kernel.provider.retriesExhausted": "여러 번 다시 시도했지만 실패했습니다.",
  "kernel.provider.quota": "제공자 계정의 사용 한도가 부족합니다.",
  "kernel.provider.modelNotFound": "제공자가 이 모델을 인식하지 못합니다. 설정의 모델 이름을 확인하세요.",
  "kernel.provider.refused": "제공자가 요청을 거부했습니다(HTTP {status}).",
  "kernel.provider.htmlResponse":
    "제공자가 API 응답 대신 웹 페이지를 반환했습니다(HTTP {status}). 게이트웨이나 방화벽에 차단되었거나 API 주소가 잘못되었을 수 있습니다.",
  "kernel.provider.refusedInStream": "제공자가 응답 도중 오류를 보냈습니다.",
  "kernel.provider.transport": "제공자에 연결할 수 없습니다 — 네트워크, 프록시 또는 base URL을 확인하세요.",
  "kernel.provider.unreachable": "여러 번 시도했지만 제공자에 연결할 수 없습니다 — 네트워크, 프록시 또는 base URL을 확인하세요.",
  "kernel.provider.timeout": "요청 시간이 초과되었습니다. 네트워크나 제공자가 제때 응답하지 않았습니다. 잠시 후 다시 시도하면 보통 해결됩니다.",
  "kernel.provider.streamBroken": "제공자 응답을 읽는 도중 연결이 끊어졌습니다.",
  "kernel.provider.idle": "제공자가 {secs}초 동안 아무 데이터도 보내지 않아 이 요청을 종료했습니다. 보통 중간 프록시나 게이트웨이의 문제입니다.",
  "kernel.provider.contextOverflow": "컨텍스트 초과: {used} 사용, 한도 {limit}.",
  "kernel.provider.outputLimit": "출력 token이 소진되었습니다.",
  "kernel.provider.outputLimitExhausted": "출력 token이 {count}회 연속 소진되었습니다. 작업에 필요한 출력이 모델의 한계를 넘습니다.",
  "kernel.provider.mediaTooLarge": "첨부 파일이 너무 큽니다({bytes}바이트).",

  // ── Hooks ───────────────────────────────────────────────────
  "kernel.hook.promptBlocked": "메시지가 UserPromptSubmit hook에 의해 차단되었습니다: {reason}",
  "kernel.hook.stopBlocked": "Stop hook이 계속 진행을 요구합니다: {reason}",
  "kernel.hook.badShape": "hooks.json의 구조가 올바르지 않습니다.",

  // ── MCP ─────────────────────────────────────────────────────
  "kernel.mcp.notRunning": "「{id}」라는 MCP 서버가 실행 중이 아닙니다. 먼저 설정에서 사용하도록 설정하세요.",
  "kernel.mcp.commandNotFound":
    "시작 실패: 명령 「{command}」을(를) 찾을 수 없습니다. Finder나 Dock에서 열면 터미널의 PATH가 없으므로, 명령을 `which {command}`가 출력하는 절대 경로로 바꾸거나 npx / uvx / node가 설치되어 있는지 확인하세요.",
  "kernel.mcp.spawnFailed": "시작에 실패했습니다. 명령 경로와 인수를 확인하세요.",
  "kernel.mcp.disconnected": "프로세스가 종료되었거나 연결이 끊어졌습니다. 「다시 연결」을 눌러 다시 시도하세요.",
  "kernel.mcp.stopped": "중지되었습니다.",
  "kernel.mcp.closed": "연결이 끊어졌습니다(서버 프로세스가 종료되었을 수 있습니다).",
  "kernel.mcp.timeout": "{method}이(가) {secs}초 동안 응답하지 않았습니다.",
  "kernel.mcp.serverError": "서버가 오류를 반환했습니다({code}).",
  "kernel.mcp.cancelled": "취소되었습니다.",
  "kernel.mcp.badResponse": "서버의 응답이 예상한 형태가 아닙니다.",

  // ── 예약 작업 ────────────────────────────────────────────────
  "kernel.schedule.unavailable": "이 환경에는 예약 작업 스케줄러가 연결되어 있지 않아 예약 작업을 만들 수 없습니다.",
  "kernel.schedule.hostUnavailable": "호스트의 스케줄러에 연결할 수 없습니다.",
  "kernel.schedule.hostRejected": "호스트가 이 예약 작업 요청을 받아들이지 않았습니다.",

  // ── 하위 agent (Task) ────────────────────────────────────────
  "kernel.task.activity.started": "시작",
  "kernel.task.activity.tool": "→ {name}",
  "kernel.task.activity.said": "{text}",
  "kernel.task.activity.completed": "완료",
  "kernel.task.activity.failed": "실패",
  "kernel.task.activity.cancelled": "중지됨",
  "kernel.task.activity.interrupted": "Riot 재시작으로 중단됨",
  "kernel.task.started": "[{kind}·{model}] {title} 시작",
  "kernel.task.startedBackground": "[{kind}·{model}] {title} 백그라운드에서 시작",
  "kernel.task.launched": "{title}(이)가 백그라운드에서 시작되었습니다({id})",
  "kernel.task.completed": "{title} 완료 · {model} · {tokens} tokens · 도구 호출 {count}회",

  // ── Skills 및 명령 (설정 페이지) ──────────────────────────────
  "kernel.skill.noFrontmatter": "frontmatter가 없습니다: 파일은 ---로 시작해야 하며 그 안에 description을 한 줄 이상 적어야 합니다.",
  "kernel.skill.unterminatedFrontmatter": "frontmatter를 닫는 ---가 없습니다.",
  "kernel.skill.noDescription": "description이 없습니다 — 모델이 이 Skill을 불러올지 판단하는 유일한 근거입니다.",
  "kernel.skill.emptyBody": "본문이 비어 있습니다: frontmatter 뒤에 이 Skill의 구체적인 방법을 적어야 합니다.",
  "kernel.slash.compact": "대화 기록을 요약으로 압축하여 컨텍스트 윈도우를 확보합니다",

  // ── 설정 (설정 페이지) ────────────────────────────────────────
  "kernel.config.unreadable": "파일을 읽을 수 없습니다.",
  "kernel.config.badJson": "올바른 JSON이 아닙니다.",
  "kernel.config.encode": "설정을 직렬화할 수 없습니다.",
  "kernel.config.mcpImportShape": '구조가 올바르지 않습니다. 예상 형식: {"mcpServers": {"이름": {"command": …}}}',
  "kernel.config.mcpImportEmpty": "서버가 하나도 없습니다.",
  "kernel.config.mcpImportUnnamed": '구조가 올바르지 않습니다. 각 서버에는 이름이 있어야 합니다: {"mcpServers": {"이름": {"command": …}}}',
  "kernel.config.mcpServerBad": "「{name}」을(를) 해석할 수 없습니다.",
  "kernel.config.mcpRemoteUnsupported": "「{name}」은(는) http/sse 원격 서버입니다. Riot은 현재 stdio(command + args)만 지원합니다.",
  "kernel.config.mcpMissingCommand": "「{name}」에 command가 없습니다.",
  "kernel.config.mcpIdClash": "「{name}」의 id가 정규화 후 다른 서버와 겹칩니다({id}). 이름을 바꿔 주세요.",
  "kernel.config.mcpIdEmpty": "MCP 서버의 id는 비워 둘 수 없습니다.",
  "kernel.config.mcpIdInvalid": "MCP 서버 id 「{id}」에는 영문자, 숫자, -, _만 쓸 수 있습니다(도구 이름에 들어가기 때문입니다).",
  "kernel.config.mcpIdDuplicate": "MCP 서버 id 「{id}」이(가) 중복되었습니다.",
  "kernel.config.promptIdEmpty": "프롬프트의 id는 비워 둘 수 없습니다.",
  "kernel.config.promptIdDuplicate": "프롬프트 id 「{id}」이(가) 중복되었습니다.",
  "kernel.config.noProvider": "아직 제공자가 설정되지 않았습니다. 설정에서 하나 추가하세요.",
  "kernel.config.providerNotFound": "제공자 「{id}」를 찾을 수 없습니다.",
  "kernel.config.noModelSelected":
    "「{name}」에 아직 선택된 모델이 없습니다. 설정에서 모델을 추가하고 선택하거나, 입력창의 모델 메뉴에서 선택하세요.",
} satisfies Dict<typeof zh>;
