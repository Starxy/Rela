/** 与 crates/rela-protocol 保持一致；读取模型不包含密钥，仅更新请求可携带新密钥。 */
export interface ConnectionStatus {
  connected: boolean;
  core: 'unavailable' | 'stopped' | 'starting' | 'stopping' | 'running';
  virtual_ip: string | null;
  gateway: 'unknown' | 'online' | 'offline';
  latency_ms: number | null;
  connection_type: 'direct' | 'relay' | null;
  resources_available: number;
  resources_total: number;
  last_error: string | null;
}

export interface LabResource {
  id: string;
  name: string;
  kind: 'ssh' | 'web' | 'nas';
  description: string;
  address: string;
  availability: 'unknown' | 'reachable' | 'unreachable';
}

export interface DiagnosticCheck {
  id: string;
  label: string;
  level: 'pass' | 'warning' | 'error' | 'skipped';
  message: string;
}

export interface DiagnosticReport {
  generated_at: string;
  summary: string;
  checks: DiagnosticCheck[];
}

export interface Preferences {
  device_name: string;
  launch_at_login: boolean;
  auto_connect: boolean;
}

export interface VersionInfo {
  app: string;
  easytier_target: string;
  easytier_bundled: string | null;
  easytier_deployed: string | null;
  easytier_running: string | null;
  engine_owner_app: string | null;
  engine_revision: number | null;
  protocol: number;
}

export type UpdateChannel = 'stable' | 'test';
export interface SoftwareUpdateStatus {
  channel: UpdateChannel;
  current_version: string;
  checking: boolean;
  last_checked: string | null;
  last_error: string | null;
  candidate: {
    version: string;
    notes: string;
    published_at: string;
    release_url: string;
  } | null;
  cached: boolean;
}

export interface NetworkConfig {
  network_name: string;
  has_credential: boolean;
  peers: string[];
  private_mode: boolean;
  disable_p2p: boolean;
  gateway_ip: string | null;
}

export interface NetworkConfigUpdate {
  network_name: string;
  credential_secret?: string;
  peers: string[];
  private_mode: boolean;
  disable_p2p: boolean;
  gateway_ip: string | null;
}

export interface ResourceSyncStatus {
  local_override: boolean;
  resource_version: number | null;
  applied_resource_version: number | null;
  pending_reconnect: boolean;
  configuration_ready: boolean;
  credential_unavailable: boolean;
  has_credential: boolean;
  refreshing: boolean;
  last_checked: string | null;
  last_error: string | null;
}

export function coreIsActive(status: ConnectionStatus | null): boolean {
  return (
    status != null && ['running', 'starting', 'stopping'].includes(status.core)
  );
}

export interface RelaService {
  mode: 'desktop' | 'preview';
  getStatus(): Promise<ConnectionStatus>;
  connect(): Promise<ConnectionStatus>;
  disconnect(): Promise<ConnectionStatus>;
  reconnect(): Promise<ConnectionStatus>;
  getResources(): Promise<LabResource[]>;
  openResource(id: string): Promise<void>;
  runDiagnostics(): Promise<DiagnosticReport>;
  exportLogs(): Promise<string>;
  getVersion(): Promise<VersionInfo>;
  getPreferences(): Promise<Preferences>;
  savePreferences(preferences: Preferences): Promise<Preferences>;
  getNetworkConfig(): Promise<NetworkConfig>;
  saveNetworkConfig(config: NetworkConfigUpdate): Promise<NetworkConfig>;
  resetNetworkConfig(): Promise<NetworkConfig>;
  getResourceSync(): Promise<ResourceSyncStatus>;
  refreshResources(): Promise<ResourceSyncStatus>;
  getSoftwareUpdate(): Promise<SoftwareUpdateStatus>;
  checkSoftwareUpdate(): Promise<SoftwareUpdateStatus>;
  setUpdateChannel(channel: UpdateChannel): Promise<SoftwareUpdateStatus>;
  openSoftwareRelease(version: string): Promise<void>;
}

export function errorMessage(error: unknown): string {
  if (typeof error === 'object' && error !== null && 'code' in error) {
    const messages: Record<string, string> = {
      core_unavailable: '网络引擎不可用',
      not_supported: '暂不可用',
      resource_unavailable: '资源不可用',
    };
    const message = messages[String(error.code)];
    if (message) return message;
  }
  if (error instanceof Error) return error.message;
  if (typeof error === 'object' && error !== null && 'message' in error) {
    return String(error.message);
  }
  return typeof error === 'string' ? error : '操作未完成，请稍后重试。';
}
