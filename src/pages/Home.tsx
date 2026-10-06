import {
  Activity,
  ChevronRight,
  Info,
  LoaderCircle,
  Power,
  Server,
} from 'lucide-react';
import type { ConnectionStatus, ResourceSyncStatus } from '../types';
import { coreIsActive } from '../types';
import {
  connectionLabel,
  connectionTypeLabel,
  gatewayLabel,
  latencyLabel,
  latencyPlaceholder,
  metricsDescription,
} from '../connection-status';

interface Props {
  status: ConnectionStatus | null;
  sync: ResourceSyncStatus | null;
  busy: string | null;
  error: string | null;
  onConnect: () => void;
  onDiagnose: () => void;
  onResources: () => void;
  onDetails: () => void;
}

export function Home({
  status,
  sync,
  busy,
  error,
  onConnect,
  onDiagnose,
  onResources,
  onDetails,
}: Props) {
  const connected = status?.connected ?? false;
  const active = coreIsActive(status);
  const pending = busy === 'connection';
  const connecting =
    !error && active && !connected && status?.core !== 'stopping';
  const unavailable =
    !active && (!sync?.configuration_ready || !sync.has_credential);
  const label = pending
    ? active
      ? '正在断开'
      : '正在连接'
    : !error && status?.core === 'stopped' && !sync?.configuration_ready
      ? '等待获取网络配置'
      : !error && status?.core === 'stopped' && !sync?.has_credential
        ? '请填写连接凭据'
        : connectionLabel(status, error);

  return (
    <main className="home">
      <section
        className={`connection-control ${connected ? 'is-connected' : ''}`}
        aria-label="连接状态"
      >
        <div className="power-ring">
          <button
            className={`power-switch ${pending || connecting ? 'is-pending' : ''}`}
            role="switch"
            aria-checked={active}
            aria-label="实验室连接"
            aria-busy={pending || connecting}
            title={active ? '断开连接' : '连接'}
            disabled={
              !!busy || !status || status.core === 'stopping' || unavailable
            }
            onClick={onConnect}
          >
            {pending || connecting ? (
              <LoaderCircle
                className="spin"
                size={38}
                strokeWidth={1.7}
                aria-hidden="true"
              />
            ) : (
              <Power size={38} strokeWidth={1.7} aria-hidden="true" />
            )}
          </button>
        </div>
        <h2 className="connection-label" role="status">
          <i aria-hidden="true" />
          {label}
        </h2>
        <div
          className={`connection-address ${connected ? '' : 'connection-hint'}`}
        >
          {connected
            ? status?.virtual_ip
            : error ||
              (active || status?.core === 'unavailable'
                ? status?.last_error
                : null)}
        </div>
      </section>
      <dl className="metrics" aria-label="网络状态">
        <div>
          <dt>校园网关</dt>
          <dd className={status?.gateway === 'online' ? 'positive' : ''}>
            {gatewayLabel(status)}
          </dd>
        </div>
        <div>
          <dt>{latencyLabel(status)}</dt>
          <dd title={metricsDescription(status)}>
            {status?.latency_ms != null ? (
              <>
                {status.latency_ms}
                <span className="unit"> ms</span>
              </>
            ) : (
              latencyPlaceholder(status)
            )}
          </dd>
        </div>
        <div>
          <dt>连接方式</dt>
          <dd title={metricsDescription(status)}>
            {connectionTypeLabel(status)}
          </dd>
        </div>
      </dl>
      <button className="resource-entry" onClick={onResources}>
        <span className="entry-icon">
          <Server size={18} aria-hidden="true" />
        </span>
        <span>实验室资源</span>
        <span className="resource-count">
          {status ? (
            <>
              <strong>{status.resources_available}</strong>
              <span> / {status.resources_total}</span>
            </>
          ) : (
            '—'
          )}
        </span>
        <ChevronRight size={15} aria-hidden="true" />
      </button>
      <footer className="quick-actions">
        <button
          onClick={onDiagnose}
          disabled={!!busy && busy !== 'diagnostics'}
        >
          <Activity size={16} aria-hidden="true" />
          诊断
        </button>
        <span className="action-divider" aria-hidden="true" />
        <button onClick={onDetails}>
          <Info size={16} aria-hidden="true" />
          连接详情
        </button>
      </footer>
    </main>
  );
}
