import {
  Check,
  CircleAlert,
  Copy,
  Download,
  LoaderCircle,
  Minus,
  RotateCw,
  X,
} from 'lucide-react';
import { useState } from 'react';
import type { DiagnosticReport } from '../types';

export function Diagnostics({
  report,
  busy,
  onRun,
  onExport,
}: {
  report: DiagnosticReport | null;
  busy: string | null;
  onRun: () => void;
  onExport: () => void;
}) {
  const [copyState, setCopyState] = useState<{
    report: DiagnosticReport;
    label: string;
  } | null>(null);
  const copy = async () => {
    if (!report) return;
    try {
      await navigator.clipboard.writeText(
        [
          report.summary,
          ...report.checks.map((check) => `${check.label}：${check.message}`),
        ].join('\n'),
      );
      setCopyState({ report, label: '已复制' });
    } catch {
      setCopyState({ report, label: '复制失败' });
    }
  };
  const running = busy === 'diagnostics';

  return (
    <section className="diagnostics" aria-label="诊断结果">
      {running ? (
        <div className="diagnostics-loading" role="status">
          <LoaderCircle className="spin" size={24} aria-hidden="true" />
          检查中…
        </div>
      ) : report ? (
        <>
          <p className="diagnostic-summary" role="status">
            {report.summary}
          </p>
          <ul className="diagnostic-checks">
            {report.checks.map((check) => {
              const Icon =
                check.level === 'pass'
                  ? Check
                  : check.level === 'error'
                    ? X
                    : check.level === 'warning'
                      ? CircleAlert
                      : Minus;
              return (
                <li key={check.id}>
                  <span className={`check-icon ${check.level}`}>
                    <Icon size={17} aria-hidden="true" />
                  </span>
                  <div>
                    <strong>{check.label}</strong>
                    <p>{check.message}</p>
                  </div>
                </li>
              );
            })}
          </ul>
        </>
      ) : (
        <p className="empty-state">暂无结果</p>
      )}
      <div className="diagnostic-actions">
        <button className="button secondary" onClick={onRun} disabled={!!busy}>
          <RotateCw size={14} aria-hidden="true" />
          重试
        </button>
        <button
          className="button secondary"
          onClick={() => void copy()}
          disabled={!!busy || !report}
        >
          <Copy size={14} aria-hidden="true" />
          {copyState?.report === report ? copyState?.label : '复制'}
        </button>
        <button
          className="button secondary"
          onClick={onExport}
          disabled={!!busy || !report}
        >
          <Download size={14} aria-hidden="true" />
          导出
        </button>
      </div>
    </section>
  );
}
