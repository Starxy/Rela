import type { ConnectionStatus } from './types';

export function connectionLabel(
  status: ConnectionStatus | null,
  error: string | null = null,
): string {
  if (error) return '状态未知';
  if (!status) return '正在加载';
  if (status.core === 'stopping') return '正在断开';
  if (status.connected) return '已连接';
  if (status.core === 'starting' || status.core === 'running')
    return '正在连接';
  if (status.core === 'unavailable') return '引擎不可用';
  return '未连接';
}

export function gatewayLabel(status: ConnectionStatus | null): string {
  if (status?.gateway === 'online') return '正常';
  if (status?.gateway === 'offline') return '未响应';
  if (status?.gateway === 'not_configured') return '未设置';
  return status?.connected ? '检测中' : '—';
}

export function connectionTypeLabel(status: ConnectionStatus | null): string {
  if (status?.connection_type === 'direct') return '直连';
  if (status?.connection_type === 'relay') return '中继';
  return status?.connected ? '检测中' : '—';
}

export function latencyLabel(status: ConnectionStatus | null): string {
  return status?.gateway === 'not_configured' ? '节点延迟' : '延迟';
}

export function latencyPlaceholder(status: ConnectionStatus | null): string {
  if (status?.gateway === 'offline') return '超时';
  if (status?.connected && status.metrics_target) return '未响应';
  return status?.connected ? '检测中' : '—';
}

export function metricsDescription(status: ConnectionStatus | null): string {
  if (!status?.metrics_target) return '等待网络引擎返回检测数据';
  return status.gateway === 'not_configured'
    ? `到网络节点 ${status.metrics_target} 的延迟和路由方式`
    : `到校园网关 ${status.metrics_target} 的 ICMP 延迟和路由方式`;
}
