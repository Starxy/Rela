import { useEffect, useState } from 'react';
import { networkConfigUpdate } from '../services/network-config';
import type { ConnectionStatus, NetworkConfig, RelaService } from '../types';
import { coreIsActive, errorMessage } from '../types';

export function NetworkSettings({
  service,
  status,
  onSaved,
}: {
  service: RelaService;
  status: ConnectionStatus | null;
  onSaved: () => void;
}) {
  const [config, setConfig] = useState<NetworkConfig | null>(null);
  const [secret, setSecret] = useState('');
  const [peers, setPeers] = useState('');
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  const active = coreIsActive(status);

  useEffect(() => {
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
  }, [service]);

  async function save(reconnect: boolean) {
    if (!config || busy) return;
    setBusy(true);
    setMessage(null);
    setFailed(false);
    try {
      const next = await service.saveNetworkConfig(
        networkConfigUpdate(config, secret, peers),
      );
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
      setConfig(next);
      setSecret('');
      setPeers(next.peers.join('\n'));
      setMessage('已恢复默认设置，下次连接时生效。');
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
      {config ? (
        <form
          className="network-form"
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
          <label className="field-label" htmlFor="network-secret">
            网络密钥
          </label>
          <input
            id="network-secret"
            className="text-input"
            type="password"
            value={secret}
            autoComplete="new-password"
            placeholder={
              config.has_network_secret
                ? '已保存，留空保持不变'
                : '请输入网络密钥'
            }
            required={!config.has_network_secret}
            maxLength={1024}
            disabled={busy}
            onChange={(event) => setSecret(event.target.value)}
          />
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
