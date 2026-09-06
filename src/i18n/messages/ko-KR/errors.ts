import type { Dict } from "..";
import type zh from "../zh-CN/errors";

export default {
  "errors.withDetail": "{text} ({detail})",
  "errors.ipcTimeout": "호스트가 응답하지 않습니다: {command}이(가) {seconds}초 동안 반환되지 않았습니다.",
  "errors.disconnected": "호스트와의 연결이 끊어졌습니다. 잠시 후 다시 시도하세요.",
  "errors.unknown": "문제가 발생했습니다.",

  "errors.authRequired": "아직 호스트에 연결되지 않았습니다: 액세스 토큰이 필요합니다",
  "errors.reconnectingWithToken": "새 토큰으로 다시 연결하는 중입니다.",
  "errors.signedOut": "로그아웃됨",
  "errors.hostDenied": "호스트가 연결을 거부했습니다: {reason}",

  "errors.webNoServerFiles": "웹 버전에서는 서버의 파일을 선택할 수 없습니다. 입력창에서 @로 참조하세요.",
  "errors.webNoLocalOpen": "웹 버전에서는 이 기기에서 서버의 파일을 열 수 없습니다.",
  "errors.webUseInAppDirPicker": "웹 버전에서는 앱 내 폴더 선택기를 사용하세요.",
  "errors.popupBlocked": "브라우저가 새 창을 차단했습니다. 팝업을 허용한 뒤 다시 시도하세요.",

  "errors.fileNotFound": "파일이 없습니다: {path}",
  "errors.dialog.imagesFilter": "이미지",
} satisfies Dict<typeof zh>;
