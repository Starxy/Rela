import { useEffect, useRef, useState } from 'react';
import { networkConfigUpdate } from '../services/network-config';
import type {
  ConnectionStatus,
  NetworkConfig,
  RelaService,
  ResourceSyncStatus,
} from '../types';
import { coreIsActive, errorMessage } from '../types';

export function NetworkSettings({
  service,
  status,
  sync,
  onSaved,
}: {
  service: RelaService;
  status: ConnectionStatus | null;
  sync: ResourceSyncStatus | null;
  onSaved: () => void;
}) {
  const [config, setConfig] = useState<NetworkConfig | null>(null);
  const [secret, setSecret] = useState('');
  const [peers, setPeers] = useState('');
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  const active = coreIsActive(status);
  const dirty = useRef(false);

  useEffect(() => {
    if (dirty.current) return;
    let mounted = true;
    service
      .getNetworkConfig()
      .then((next) => {
        if (mounted) {
          setConfig(next);
          setPeers(next.peers.join('\n'));
        }
      })
      .catch((error: unknown) => {
        if (mounted) {
          setMessage(errorMessage(error));
          setFailed(true);
        }
      });
    return () => {
      mounted = false;
    };
  }, [service, sync?.resource_version, sync?.has_credential]);

  async function save(reconnect: boolean) {
    if (!config || busy) return;
    setBusy(true);
    setMessage(null);
    setFailed(false);
    try {
      const next = await service.saveNetworkConfig(
        networkConfigUpdate(config, secret, peers),
      );
      dirty.current = false;
      setSecret('');
      setConfig(next);
      setPeers(next.peers.join('\n'));
      if (reconnect) {
        await service.reconnect();
        setMessage('设置已保存，已重新启动连接。');
      } else {
        setMessage('设置已保存，下次连接时生效。');
      }
      onSaved();
    } catch (error) {
      setMessage(errorMessage(error));
      setFailed(true);
    } finally {
      setBusy(false);
    }
  }

  async function reset() {
    if (busy) return;
    setBusy(true);
    setMessage(null);
    setFailed(false);
    try {
      const next = await service.resetNetworkConfig();
      dirty.current = false;
      setConfig(next);
      setSecret('');
      setPeers(next.peers.join('\n'));
      setMessage('已恢复默认设置，请导入 credential 后连接。');
      onSaved();
    } catch (error) {
      setMessage(errorMessage(error));
      setFailed(true);
    } finally {
      setBusy(false);
    }
  }

  async function clearCredential() {
    if (!config || busy || active) return;
    setBusy(true);
    setMessage(null);
    setFailed(false);
    try {
      const next = await service.saveNetworkConfig({
        ...networkConfigUpdate(config, '', peers),
        credential_secret: '',
      });
      dirty.current = false;
      setSecret('');
      setConfig(next);
      setPeers(next.peers.join('\n'));
      setMessage('已清除本地凭据，重新导入后才能连接。');
      onSaved();
    } catch (error) {
      setMessage(errorMessage(error));
      setFailed(true);
    } finally {
      setBusy(false);
    }
  }

  async function refreshOnline() {
    if (busy || sync?.refreshing) return;
    setBusy(true);
    setFailed(false);
    setMessage(null);
    try {
      const updated = await service.refreshResources();
      const next = await service.getNetworkConfig();
      dirty.current = false;
      setConfig(next);
      setPeers(next.peers.join('\n'));
      setSecret('');
      setMessage(
        updated.pending_reconnect
          ? '线上配置已更新，重新连接后使用新线路。'
          : '线上配置已更新，已恢复自动刷新。',
      );
      onSaved();
    } catch (error) {
      setMessage(errorMessage(error));
      setFailed(true);
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <div className="config-sync">
        <div className="config-sync-heading">
          <strong>
            {sync?.local_override ? '使用本地线路' : '使用线上默认配置'}
          </strong>
          <span>
            {sync?.resource_version != null
              ? `资源版本 ${sync.resource_version}`
              : '等待获取'}
          </span>
        </div>
        <p className="field-hint">
          {sync?.local_override
            ? '自动获取已暂停。手动更新会覆盖网络名称、节点和资源。'
            : '启动时自动获取网络与资源，本机开关保持不变。'}
        </p>
        <button
          className="button"
          type="button"
          disabled={busy || sync?.refreshing}
          onClick={() => void refreshOnline()}
        >
          {sync?.refreshing ? '正在获取…' : '更新线上配置'}
        </button>
        {sync?.last_error && !message && (
          <p className="form-message" role="status">
            {sync.last_error}
          </p>
        )}
        {sync?.credential_unavailable && (
          <p className="form-message" role="status">
            当前用户无法读取已存凭据，请重新填写。
          </p>
        )}
      </div>
      {config ? (
        <form
          className="network-form"
          onChange={() => {
            dirty.current = true;
          }}
          onSubmit={(event) => {
            event.preventDefault();
            void save(false);
          }}
        >
          <label className="field-label" htmlFor="network-name">
            网络名称
          </label>
          <input
            id="network-name"
            className="text-input"
            value={config.network_name}
            maxLength={128}
            required
            disabled={busy}
            onChange={(event) =>
              setConfig({ ...config, network_name: event.target.value })
            }
          />
          <label className="field-label" htmlFor="credential-secret">
            credential 凭据
          </label>
          <input
            id="credential-secret"
            className="text-input"
            type="password"
            value={secret}
            autoComplete="new-password"
            placeholder={
              config.has_credential
                ? '已保存，留空保持不变'
                : '粘贴管理员签发的 credential'
            }
            maxLength={128}
            spellCheck={false}
            disabled={busy}
            onChange={(event) => setSecret(event.target.value)}
          />
          <p className="field-hint">
            {config.has_credential
              ? '凭据已保存，填写新值可更换。'
              : '尚未导入凭据，暂时无法连接。'}
          </p>
          {config.has_credential && (
            <button
              className="text-button"
              type="button"
              disabled={busy || active}
              title={active ? '请先断开连接再清除凭据' : undefined}
              onClick={() => void clearCredential()}
            >
              清除凭据
            </button>
          )}
          <label className="field-label" htmlFor="network-peers">
            连接节点
          </label>
          <textarea
            id="network-peers"
            className="text-input peer-input"
            value={peers}
            rows={2}
            maxLength={32768}
            required
            disabled={busy}
            spellCheck={false}
            placeholder="tcp://服务器:11010"
            onChange={(event) => setPeers(event.target.value)}
          />
          <p className="field-hint">多个节点每行填写一个。</p>
          <div className="settings-toggles network-toggles">
            <label className="toggle-row">
              <span>私有模式</span>
              <input
                type="checkbox"
                role="switch"
                checked={config.private_mode}
                disabled={busy}
                onChange={(event) =>
                  setConfig({ ...config, private_mode: event.target.checked })
                }
              />
              <span className="toggle" aria-hidden="true" />
            </label>
            <label className="toggle-row">
              <span>禁用 P2P</span>
              <input
                type="checkbox"
                role="switch"
                checked={config.disable_p2p}
                disabled={busy}
                onChange={(event) =>
                  setConfig({ ...config, disable_p2p: event.target.checked })
                }
              />
              <span className="toggle" aria-hidden="true" />
            </label>
          </div>
          <details className="network-advanced">
            <summary>网关检测</summary>
            <label className="field-label" htmlFor="gateway-ip">
              校园网关 IP（可选）
            </label>
            <input
              id="gateway-ip"
              className="text-input"
              value={config.gateway_ip ?? ''}
              placeholder="留空暂不检测"
              disabled={busy}
              onChange={(event) =>
                setConfig({ ...config, gateway_ip: event.target.value })
              }
            />
          </details>
          {message && (
            <p
              className={`form-message ${failed ? '' : 'success-message'}`}
              role={failed ? 'alert' : 'status'}
            >
              {message}
            </p>
          )}
          <div className="network-actions">
            <button className="button primary" type="submit" disabled={busy}>
              {busy ? '处理中…' : '保存'}
            </button>
            {active && (
              <button
                className="button"
                type="button"
                disabled={busy}
                onClick={() => void save(true)}
              >
                保存并重连
              </button>
            )}
          </div>
          <button
            className="text-button"
            type="button"
            disabled={busy}
            onClick={() => void reset()}
          >
            恢复默认设置
          </button>
        </form>
      ) : (
        <div className="empty-state">
          <p role={failed ? 'alert' : 'status'}>{message ?? '加载中…'}</p>
          {failed && (
            <button
              className="button"
              disabled={busy}
              onClick={() => void reset()}
            >
              恢复默认设置
            </button>
          )}
        </div>
      )}
    </>
  );
}
