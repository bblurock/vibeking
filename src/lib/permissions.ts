import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type SettingsPane = "accessibility" | "input-monitoring" | "microphone";

export type PermissionsStatus = {
  accessibility: boolean;
  input_monitoring: boolean;
  microphone: boolean;
};

export async function checkPermissions(): Promise<PermissionsStatus> {
  return invoke<PermissionsStatus>("check_permissions");
}

export async function openSettingsPane(pane: SettingsPane): Promise<void> {
  await invoke<void>("open_settings_pane", { pane });
}

export async function subscribeEventTapFailed(
  cb: () => void,
): Promise<UnlistenFn> {
  return await listen<unknown>("permissions:event-tap-failed", () => cb());
}
