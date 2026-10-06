import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { ConnectionDetails } from '../components/ConnectionDetails';
import type { ConnectionStatus, ResourceSyncStatus } from '../types';
import { Home } from './Home';

const status: ConnectionStatus = {
  connected: true,
  core: 'running',
  virtual_ip: '192.168.200.23',
  gateway: 'not_configured',
  latency_ms: 12,
  connection_type: 'direct',
  metrics_target: '192.168.200.1',
  resources_available: 0,
  resources_total: 1,
  last_error: null,
};

const sync: ResourceSyncStatus = {
  local_override: false,
  resource_version: 1,
  applied_resource_version: 1,
  pending_reconnect: false,
  configuration_ready: true,
  credential_unavailable: false,
  has_credential: true,
  refreshing: false,
  last_checked: null,
  last_error: null,
};

function home(
  nextStatus: ConnectionStatus | null,
  error: string | null = null,
) {
  const noop = () => {};
  return renderToStaticMarkup(
    <Home
      status={nextStatus}
      error={error}
      sync={sync}
      busy={null}
      onConnect={noop}
      onDiagnose={noop}
      onResources={noop}
      onDetails={noop}
    />,
  );
}

describe('首页连接状态与指标', () => {
  it('未填写可选网关时仍显示节点延迟、实际路由和检测目标', () => {
    const html = home(status);
    expect(html).toContain('已连接');
    expect(html).toContain('未设置');
    expect(html).toContain('节点延迟');
    expect(html).toContain('12<span class="unit"> ms</span>');
    expect(html).toContain('直连');
    expect(html).toContain('到网络节点 192.168.200.1');
    const details = renderToStaticMarkup(
      <ConnectionDetails status={status} error={null} />,
    );
    expect(details).toContain('<dt>检测目标</dt>');
    expect(details).toContain('192.168.200.1');
  });

  it('连接过程中首页和详情一致，并展示等待原因且允许断开', () => {
    const next = {
      ...status,
      connected: false,
      virtual_ip: null,
      metrics_target: null,
      latency_ms: null,
      connection_type: null,
      last_error: '正在获取虚拟 IP。',
    };
    const html = home(next);
    expect(html).toContain('正在连接');
    expect(html).toContain('正在获取虚拟 IP。');
    expect(html).toContain('title="断开连接"');
    expect(html).toContain('aria-busy="true"');
    expect(html).not.toContain('disabled=""');
    expect(html).not.toContain('连接未就绪');
    expect(
      renderToStaticMarkup(<ConnectionDetails status={next} error={null} />),
    ).toContain('正在连接');
  });

  it('状态读取失败清除旧的成功提示，网关超时保留真实路由', () => {
    expect(home(null, '无法读取状态')).toContain('状态未知');
    const next = {
      ...status,
      gateway: 'offline' as const,
      latency_ms: null,
      connection_type: 'relay' as const,
    };
    const html = home(next);
    expect(html).toContain('未响应');
    expect(html).toContain('超时');
    expect(html).toContain('中继');
    expect(home({ ...status, latency_ms: 0 })).toContain(
      '0<span class="unit"> ms</span>',
    );
  });
});
