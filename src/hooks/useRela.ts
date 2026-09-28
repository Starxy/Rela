import { useCallback, useEffect, useRef, useState } from 'react';
import type {
  ConnectionStatus,
  DiagnosticReport,
  LabResource,
  RelaService,
} from '../types';
import { errorMessage } from '../types';

export function useRela(service: RelaService) {
  const [status, setStatus] = useState<ConnectionStatus | null>(null);
  const [resources, setResources] = useState<LabResource[]>([]);
  const [report, setReport] = useState<DiagnosticReport | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const busyRef = useRef(false);
  const generation = useRef(0);

  const refresh = useCallback(async () => {
    const request = ++generation.current;
    try {
      const [nextStatus, nextResources] = await Promise.all([
        service.getStatus(),
        service.getResources(),
      ]);
      if (request !== generation.current) return;
      setStatus(nextStatus);
      setResources(nextResources);
      setError(null);
    } catch (cause) {
      if (request !== generation.current) return;
      // 旧的“已连接”状态在状态查询失败后不再可信。
      setStatus(null);
      setResources([]);
      setError(errorMessage(cause));
    }
  }, [service]);

  useEffect(() => {
    // refresh 在 Native/演示服务返回后才更新状态，不会在 effect 中同步 setState。
    // eslint-disable-next-line react-hooks/set-state-in-effect
    void refresh();
    const timer = window.setInterval(() => {
      if (!busyRef.current) void refresh();
    }, 5000);
    return () => {
      window.clearInterval(timer);
      generation.current += 1;
    };
  }, [refresh]);

  const perform = async (name: string, action: () => Promise<void>) => {
    if (busyRef.current) return;
    busyRef.current = true;
    generation.current += 1;
    setBusy(name);
    setNotice(null);
    try {
      await action();
    } catch (cause) {
      setNotice(errorMessage(cause));
    } finally {
      busyRef.current = false;
      setBusy(null);
    }
  };

  return {
    status,
    resources,
    report,
    busy,
    notice,
    error,
    dismissNotice: () => setNotice(null),
    toggleConnection: () =>
      perform('connection', async () => {
        if (status?.connected) await service.disconnect();
        else await service.connect();
        setReport(null);
        await refresh();
      }),
    diagnose: () =>
      perform('diagnostics', async () => {
        setReport(await service.runDiagnostics());
      }),
    exportLogs: () =>
      perform('export', async () => {
        setNotice(await service.exportLogs());
      }),
    openResource: (id: string) =>
      perform('resource', () => service.openResource(id)),
  };
}
