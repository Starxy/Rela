import type { ConnectionStatus } from '../types';

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
          <dd>{status ? (status.connected ? '已连接' : '未连接') : '未知'}</dd>
        </div>
        <div>
          <dt>虚拟 IP</dt>
          <dd className="mono">{status?.virtual_ip ?? '—'}</dd>
        </div>
        <div>
          <dt>校园网关</dt>
          <dd>
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
            {status?.latency_ms != null ? `${status.latency_ms} ms` : '—'}
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
        <div>
          <dt>网络引擎</dt>
          <dd>
            {status?.core === 'running'
              ? '运行中'
              : status?.core === 'stopped'
                ? '已停止'
                : status?.core === 'unavailable'
                  ? '不可用'
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
      {error && (
        <p className="form-message" role="alert">
          {error}
        </p>
      )}
    </>
  );
}
