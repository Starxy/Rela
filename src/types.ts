/** 与 crates/rela-protocol 中的业务模型保持一致；此处禁止出现网络凭据。 */
export interface ConnectionStatus {
  connected: boolean;
  core: 'unavailable' | 'stopped' | 'running';
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
  easytier_installed: string | null;
  protocol: number;
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
