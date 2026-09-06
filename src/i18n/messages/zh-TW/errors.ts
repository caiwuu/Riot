import type { Dict } from "..";
import type zh from "../zh-CN/errors";

export default {
  "errors.withDetail": "{text}（{detail}）",
  "errors.ipcTimeout": "主機沒有回應：{command} 超過 {seconds} 秒沒有回傳。",
  "errors.disconnected": "與主機的連線已中斷，請稍後再試。",
  "errors.unknown": "出了點問題。",

  "errors.authRequired": "尚未連上主機：需要存取權杖",
  "errors.reconnectingWithToken": "正在用新權杖重新連線。",
  "errors.signedOut": "已登出",
  "errors.hostDenied": "主機拒絕連線：{reason}",

  "errors.webNoServerFiles": "網頁版不能選取伺服器上的檔案，請在輸入框裡用 @ 引用它。",
  "errors.webNoLocalOpen": "網頁版無法在這台裝置上開啟伺服器上的檔案。",
  "errors.webUseInAppDirPicker": "網頁版請使用應用程式內的資料夾選擇器。",
  "errors.popupBlocked": "瀏覽器封鎖了新視窗，請允許彈出視窗後再試。",

  "errors.fileNotFound": "檔案不存在：{path}",
  "errors.dialog.imagesFilter": "圖片",
} satisfies Dict<typeof zh>;
