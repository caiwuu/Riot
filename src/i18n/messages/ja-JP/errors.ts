import type { Dict } from "..";
import type zh from "../zh-CN/errors";

export default {
  "errors.withDetail": "{text}（{detail}）",
  "errors.ipcTimeout": "ホストが応答しません：{command} が {seconds} 秒経っても返ってきません。",
  "errors.disconnected": "ホストとの接続が切れました。しばらくしてから再試行してください。",
  "errors.unknown": "問題が発生しました。",

  "errors.authRequired": "ホストに未接続です：アクセストークンが必要です",
  "errors.reconnectingWithToken": "新しいトークンで再接続しています。",
  "errors.signedOut": "サインアウトしました",
  "errors.hostDenied": "ホストが接続を拒否しました：{reason}",

  "errors.webNoServerFiles": "Web 版ではサーバー上のファイルを選択できません。入力欄で @ を使って参照してください。",
  "errors.webNoLocalOpen": "Web 版では、このデバイスからサーバー上のファイルを開けません。",
  "errors.webUseInAppDirPicker": "Web 版ではアプリ内のフォルダ選択を使ってください。",
  "errors.popupBlocked": "ブラウザが新しいウィンドウをブロックしました。ポップアップを許可してから再試行してください。",

  "errors.fileNotFound": "ファイルが存在しません：{path}",
  "errors.dialog.imagesFilter": "画像",
} satisfies Dict<typeof zh>;
