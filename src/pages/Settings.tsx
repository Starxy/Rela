import { useEffect, useState } from 'react';
import type { Preferences, RelaService, VersionInfo } from '../types';
import { errorMessage } from '../types';

export function Settings({
  service,
  onSaved,
}: {
  service: RelaService;
  onSaved: () => void;
}) {
  const [preferences, setPreferences] = useState<Preferences | null>(null);
  const [version, setVersion] = useState<VersionInfo | null>(null);
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState<string | null>(null);

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
      onSaved();
    } catch (error) {
      setMessage(errorMessage(error));
    } finally {
      setSaving(false);
    }
  };

  return (
    <>
      {preferences ? (
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
      {version && (
        <div className="about-row">
          <span>Rela</span>
          <span>{version.app}</span>
        </div>
      )}
    </>
  );
}
