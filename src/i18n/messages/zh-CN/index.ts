import app from "./app";
import common from "./common";
import composer from "./composer";
import errors from "./errors";
import host from "./host";
import kernel from "./kernel";
import panels from "./panels";
import schedules from "./schedules";
import settings from "./settings";
import settingsExt from "./settingsExt";
import tools from "./tools";
import transcript from "./transcript";

export default {
  ...common,
  ...app,
  ...composer,
  ...transcript,
  ...panels,
  ...schedules,
  ...settings,
  ...settingsExt,
  ...errors,
  ...host,
  ...kernel,
  ...tools,
};
