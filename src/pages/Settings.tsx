import { useEffect, useState } from 'react';
import type {
  ConnectionStatus,
  Preferences,
  RelaService,
  VersionInfo,
} from '../types';
import { errorMessage } from '../types';
import { NetworkSettings } from '../components/NetworkSettings';

export function Settings({
  service,
  status,
  onSaved,
}: {
  service: RelaService;
  status: ConnectionStatus | null;
  onSaved: () => void;
}) {
  const [preferences, setPreferences] = useState<Preferences | null>(null);
  const [version, setVersion] = useState<VersionInfo | null>(null);
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [tab, setTab] = useState<'network' | 'general'>('network');

  useEffect(() => {
    let active = true;
    Promise.all([service.getPreferences(), service.getVersion()])
      .then(([prefs, info]) => {
        if (active) {
          setPreferences(prefs);
          setVersion(info);
        }
      })
      .catch((error: unknown) => {
        if (active) setMessage(errorMessage(error));
      });
    return () => {
      active = false;
    };
  }, [service]);

  const save = async () => {
    if (!preferences || saving) return;
    setSaving(true);
    setMessage(null);
    try {
      await service.savePreferences(preferences);
      setMessage('设置已保存。');
      onSaved();
    } catch (error) {
      setMessage(errorMessage(error));
    } finally {
      setSaving(false);
    }
  };

  return (
    <>
      <div className="settings-tabs" role="tablist" aria-label="设置分类">
        <button
          id="network-tab"
          type="button"
          role="tab"
          aria-selected={tab === 'network'}
          aria-controls="settings-panel"
          onClick={() => setTab('network')}
        >
          网络
        </button>
        <button
          id="general-tab"
          type="button"
          role="tab"
          aria-selected={tab === 'general'}
          aria-controls="settings-panel"
          onClick={() => setTab('general')}
        >
          常规
        </button>
      </div>
      <div
        id="settings-panel"
        role="tabpanel"
        aria-labelledby={tab === 'network' ? 'network-tab' : 'general-tab'}
      >
        {tab === 'network' ? (
          <NetworkSettings
            service={service}
            status={status}
            onSaved={onSaved}
          />
        ) : preferences ? (
          <form
            className="settings-form"
            onSubmit={(event) => {
              event.preventDefault();
              void save();
            }}
          >
            <label className="field-label" htmlFor="device-name">
              设备名称
            </label>
            <input
              id="device-name"
              className="text-input"
              value={preferences.device_name}
              maxLength={64}
              required
              disabled={saving}
              onChange={(event) =>
                setPreferences({
                  ...preferences,
                  device_name: event.target.value,
                })
              }
            />
            <div className="settings-toggles">
              <label className="toggle-row">
                <span>开机启动</span>
                <input
                  type="checkbox"
                  role="switch"
                  checked={preferences.launch_at_login}
                  disabled={service.mode !== 'preview' || saving}
                  onChange={(event) =>
                    setPreferences({
                      ...preferences,
                      launch_at_login: event.target.checked,
                    })
                  }
                />
                <span className="toggle" aria-hidden="true" />
              </label>
              <label className="toggle-row">
                <span>自动连接</span>
                <input
                  type="checkbox"
                  role="switch"
                  checked={preferences.auto_connect}
                  disabled={service.mode !== 'preview' || saving}
                  onChange={(event) =>
                    setPreferences({
                      ...preferences,
                      auto_connect: event.target.checked,
                    })
                  }
                />
                <span className="toggle" aria-hidden="true" />
              </label>
            </div>
            {message && (
              <p className="form-message" role="alert">
                {message}
              </p>
            )}
            <button
              className="button primary full-width"
              type="submit"
              disabled={saving}
            >
              {saving ? '保存中…' : '保存'}
            </button>
          </form>
        ) : (
          <p className="empty-state" role="status">
            {message ?? '加载中…'}
          </p>
        )}
      </div>
      {version && (
        <div className="about-row">
          <span>Rela</span>
          <span>{version.app}</span>
        </div>
      )}
    </>
  );
}
