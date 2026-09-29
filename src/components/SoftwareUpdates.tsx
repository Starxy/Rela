import { useEffect, useState } from 'react';
import type {
  RelaService,
  SoftwareUpdateStatus,
  UpdateChannel,
  UpdateProgress,
} from '../types';
import { errorMessage } from '../types';

export function SoftwareUpdates({ service }: { service: RelaService }) {
  const [status, setStatus] = useState<SoftwareUpdateStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [progress, setProgress] = useState<UpdateProgress | null>(null);
  const [confirmation, setConfirmation] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    const read = () => {
      service
        .getUpdateProgress()
        .then((value) => {
          if (active) setProgress(value);
        })
        .catch(() => {
          /* 保留最后读取的进度。 */
        });
      service
        .getSoftwareUpdate()
        .then((value) => {
          if (active) setStatus(value);
        })
        .catch((error: unknown) => {
          if (active) setMessage(errorMessage(error));
        });
    };
    read();
    const timer = window.setInterval(read, 1000);
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
  const updating =
    progress != null &&
    ['downloading', 'preparing', 'restarting'].includes(progress.phase);
  const checking = !updating && (busy || status?.checking);
  const disabled = busy || !!status?.checking || updating;
  const candidate = status?.candidate;
  const error = message ?? progress?.error ?? status?.last_error;
  const install = async () => {
    if (!confirmation || disabled) return;
    const version = confirmation;
    setConfirmation(null);
    setMessage(null);
    setBusy(true);
    setProgress({
      phase: 'downloading',
      version,
      downloaded: 0,
      total: candidate?.size ?? 0,
      error: null,
    });
    try {
      await service.installSoftwareUpdate(version);
    } catch (failure) {
      const text = errorMessage(failure);
      setMessage(text);
      setProgress({
        phase: 'failed',
        version,
        downloaded: 0,
        total: 0,
        error: text,
      });
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="software-updates" aria-label="软件更新">
      <div className="config-sync-heading">
        <strong>软件更新</strong>
        {status && (
          <span>
            {status.current_version} ·{' '}
            {status.install_kind === 'portable' ? '绿色版' : '安装版'}
          </span>
        )}
      </div>
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
          <p className="field-hint">
            下载 {(candidate.size / 1024 / 1024).toFixed(1)} MB
            {status.cached ? ' · 来自上次验证的缓存' : ''}
          </p>
          {candidate.notes && <p className="update-notes">{candidate.notes}</p>}
          {candidate.requires_manual_upgrade && (
            <p className="form-message">当前版本需要先手动升级。</p>
          )}
          {!candidate.requires_manual_upgrade && !confirmation && (
            <button
              className="button primary full-width"
              type="button"
              disabled={disabled}
              onClick={() => setConfirmation(candidate.version)}
            >
              立即更新
            </button>
          )}
        </div>
      )}
      {confirmation && (
        <div
          className="update-confirmation"
          role="group"
          aria-label="确认软件更新"
        >
          <strong>更新到 {confirmation}</strong>
          <p className="field-hint">
            确认后下载并重启 Rela。
            {status?.install_kind === 'installer'
              ? '安装更新需要管理员授权。'
              : '更新网络引擎可能需要管理员授权。'}
            更新网络引擎会短暂断开网络，请先完成正在进行的远程操作。
          </p>
          <div className="update-actions">
            <button
              className="button secondary"
              type="button"
              disabled={disabled}
              onClick={() => setConfirmation(null)}
            >
              稍后
            </button>
            <button
              className="button primary"
              type="button"
              disabled={disabled}
              onClick={() => void install()}
            >
              确认更新
            </button>
          </div>
        </div>
      )}
      {updating && progress && (
        <div className="update-progress" role="status" aria-live="polite">
          <p>
            {progress.phase === 'downloading'
              ? `正在下载 ${(progress.downloaded / 1024 / 1024).toFixed(1)} / ${(progress.total / 1024 / 1024).toFixed(1)} MB`
              : progress.phase === 'preparing'
                ? '正在验证文件并准备重启…'
                : '正在退出并启动新版 Rela…'}
          </p>
          <progress
            aria-label="更新下载进度"
            value={progress.downloaded}
            max={Math.max(progress.total, 1)}
          />
        </div>
      )}
      <p className={error ? 'form-message' : 'field-hint'} role="status">
        {updating
          ? '更新期间暂时无法修改设置或控制网络。'
          : checking
            ? '正在检查软件更新…'
            : (error ??
              (candidate
                ? '已验证更新信息。'
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
        {checking ? '正在检查…' : '检查更新'}
      </button>
    </section>
  );
}
