import { useEffect, useState } from 'react';
import type {
  RelaService,
  SoftwareUpdateStatus,
  UpdateChannel,
} from '../types';
import { errorMessage } from '../types';

export function SoftwareUpdates({ service }: { service: RelaService }) {
  const [status, setStatus] = useState<SoftwareUpdateStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    const read = () => {
      void service.getSoftwareUpdate().then(
        (value) => {
          if (active) setStatus(value);
        },
        (error: unknown) => {
          if (active) setMessage(errorMessage(error));
        },
      );
    };
    read();
    const timer = window.setInterval(read, 5000);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [service]);

  const check = async (channel?: UpdateChannel) => {
    if (busy || status?.checking) return;
    setBusy(true);
    setMessage(null);
    try {
      if (channel) setStatus(await service.setUpdateChannel(channel));
      setStatus(await service.checkSoftwareUpdate());
    } catch (error) {
      setMessage(errorMessage(error));
      try {
        setStatus(await service.getSoftwareUpdate());
      } catch {
        /* 保留最后的检查状态。 */
      }
    } finally {
      setBusy(false);
    }
  };
  const openRelease = async () => {
    if (!status?.candidate || busy) return;
    setBusy(true);
    setMessage(null);
    try {
      await service.openSoftwareRelease(status.candidate.version);
    } catch (error) {
      setMessage(errorMessage(error));
    } finally {
      setBusy(false);
    }
  };
  const candidate = status?.candidate;
  const error = message ?? status?.last_error;
  const disabled = busy || !!status?.checking;

  return (
    <section className="software-updates" aria-label="软件更新">
      <div className="config-sync-heading">
        <strong>软件更新</strong>
        {status && <span>当前版本 {status.current_version}</span>}
      </div>
      <p className="field-hint">
        发现新版本后，请前往 GitHub Release 下载并手动安装或替换。
      </p>
      <label className="update-channel" htmlFor="update-channel">
        <span>更新渠道</span>
        <select
          id="update-channel"
          value={status?.channel ?? 'stable'}
          disabled={disabled}
          onChange={(event) => void check(event.target.value as UpdateChannel)}
        >
          <option value="stable">稳定版</option>
          <option value="test">测试版</option>
        </select>
      </label>
      {status?.channel === 'test' && (
        <p className="field-hint">
          测试版用于提前验证新功能，可能存在未解决的问题。
        </p>
      )}
      {candidate && (
        <div className="update-candidate">
          <strong>发现新版本 {candidate.version}</strong>
          {status?.cached && (
            <p className="field-hint">来自上次验证的检查结果</p>
          )}
          {candidate.notes && <p className="update-notes">{candidate.notes}</p>}
          <button
            className="button primary full-width"
            type="button"
            disabled={disabled}
            onClick={() => void openRelease()}
          >
            前往 GitHub 下载
          </button>
        </div>
      )}
      <p className={error ? 'form-message' : 'field-hint'} role="status">
        {status?.checking
          ? '正在检查软件更新…'
          : (error ??
            (candidate
              ? '请下载新版本后手动更新。'
              : status?.last_checked
                ? '当前已是最新版本。'
                : '尚未检查软件更新。'))}
      </p>
      <button
        className="button secondary full-width"
        type="button"
        disabled={disabled}
        onClick={() => void check()}
      >
        {status?.checking ? '正在检查…' : '检查更新'}
      </button>
    </section>
  );
}
