import type { Dict } from "..";
import type zh from "../zh-CN/errors";

export default {
  "errors.withDetail": "{text} ({detail})",
  "errors.ipcTimeout": "The host did not respond: {command} returned nothing for {seconds} seconds.",
  "errors.disconnected": "Disconnected from the host. Please try again shortly.",
  "errors.unknown": "Something went wrong.",

  "errors.authRequired": "Not connected to the host yet: an access token is required",
  "errors.reconnectingWithToken": "Reconnecting with the new token.",
  "errors.signedOut": "Signed out",
  "errors.hostDenied": "The host refused the connection: {reason}",

  "errors.webNoServerFiles":
    "The web version cannot pick files on the server. Reference them with @ in the composer instead.",
  "errors.webNoLocalOpen": "The web version cannot open files from the server on this device.",
  "errors.webUseInAppDirPicker": "In the web version, use the in-app directory picker.",
  "errors.popupBlocked": "The browser blocked the new window. Allow pop-ups and try again.",

  "errors.fileNotFound": "File not found: {path}",
  "errors.dialog.imagesFilter": "Images",
} satisfies Dict<typeof zh>;
