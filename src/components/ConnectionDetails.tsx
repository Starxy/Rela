import type { ConnectionStatus } from '../types';
import {
  connectionLabel,
  connectionTypeLabel,
  gatewayLabel,
  latencyLabel,
  latencyPlaceholder,
  metricsDescription,
} from '../connection-status';

export function ConnectionDetails({
  status,
  error,
}: {
  status: ConnectionStatus | null;
  error: string | null;
}) {
  return (
    <>
      <dl className="detail-list">
        <div>
          <dt>连接状态</dt>
          <dd>{connectionLabel(status, error)}</dd>
        </div>
        <div>
          <dt>虚拟 IP</dt>
          <dd className="mono">{status?.virtual_ip ?? '—'}</dd>
        </div>
        <div>
          <dt>校园网关</dt>
          <dd>{gatewayLabel(status)}</dd>
        </div>
        <div>
          <dt>{latencyLabel(status)}</dt>
          <dd title={metricsDescription(status)}>
            {status?.latency_ms != null
              ? `${status.latency_ms} ms`
              : latencyPlaceholder(status)}
          </dd>
        </div>
        <div>
          <dt>连接方式</dt>
          <dd title={metricsDescription(status)}>
            {connectionTypeLabel(status)}
          </dd>
        </div>
        {status?.metrics_target && (
          <div>
            <dt>检测目标</dt>
            <dd className="mono">{status.metrics_target}</dd>
          </div>
        )}
        <div>
          <dt>网络引擎</dt>
          <dd>
            {status?.core === 'running'
              ? '运行中'
              : status?.core === 'stopped'
                ? '已停止'
                : status?.core === 'unavailable'
                  ? '不可用'
                  : status?.core === 'starting'
                    ? '正在启动'
                    : status?.core === 'stopping'
                      ? '正在停止'
                      : '—'}
          </dd>
        </div>
        <div>
          <dt>可用资源</dt>
          <dd>
            {status
              ? `${status.resources_available} / ${status.resources_total}`
              : '—'}
          </dd>
        </div>
      </dl>
      {(error || status?.last_error) && (
        <p className="form-message" role="alert">
          {error || status?.last_error}
        </p>
      )}
    </>
  );
}
