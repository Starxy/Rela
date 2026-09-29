import {
  Activity,
  ChevronRight,
  Info,
  LoaderCircle,
  Power,
  Server,
} from 'lucide-react';
import type { ConnectionStatus } from '../types';
import { coreIsActive } from '../types';

interface Props {
  status: ConnectionStatus | null;
  busy: string | null;
  error: string | null;
  onConnect: () => void;
  onDiagnose: () => void;
  onResources: () => void;
  onDetails: () => void;
}

export function Home({
  status,
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
  const label = pending
    ? active
      ? '正在断开'
      : '正在连接'
    : error
      ? '状态未知'
      : !status
        ? '正在加载'
        : status.core === 'stopping'
          ? '正在断开'
          : connected
            ? '已连接'
            : active
              ? '连接未就绪'
              : '未连接';

  return (
    <main className="home">
      <section
        className={`connection-control ${connected ? 'is-connected' : ''}`}
        aria-label="连接状态"
      >
        <div className="power-ring">
          <button
            className={`power-switch ${pending ? 'is-pending' : ''}`}
            role="switch"
            aria-checked={active}
            aria-label="实验室连接"
            aria-busy={pending}
            title={active ? '断开连接' : '连接'}
            disabled={!!busy || !status || status.core === 'stopping'}
            onClick={onConnect}
          >
            {pending ? (
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
        <div className="connection-address">
          {connected && status?.virtual_ip ? status.virtual_ip : null}
        </div>
      </section>
      <dl className="metrics" aria-label="网络状态">
        <div>
          <dt>校园网关</dt>
          <dd className={status?.gateway === 'online' ? 'positive' : ''}>
            {status?.gateway === 'online'
              ? '正常'
              : status?.gateway === 'offline'
                ? '不可达'
                : '—'}
          </dd>
        </div>
        <div>
          <dt>延迟</dt>
          <dd>
            {status?.latency_ms != null ? (
              <>
                {status.latency_ms}
                <span className="unit"> ms</span>
              </>
            ) : (
              '—'
            )}
          </dd>
        </div>
        <div>
          <dt>连接方式</dt>
          <dd>
            {status?.connection_type === 'direct'
              ? '直连'
              : status?.connection_type === 'relay'
                ? '中继'
                : '—'}
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
