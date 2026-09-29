import { useCallback, useEffect, useRef, useState } from 'react';
import type {
  ConnectionStatus,
  DiagnosticReport,
  LabResource,
  RelaService,
  ResourceSyncStatus,
} from '../types';
import { coreIsActive, errorMessage } from '../types';

export function useRela(service: RelaService) {
  const [status, setStatus] = useState<ConnectionStatus | null>(null);
  const [resources, setResources] = useState<LabResource[]>([]);
  const [sync, setSync] = useState<ResourceSyncStatus | null>(null);
  const [report, setReport] = useState<DiagnosticReport | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const busyRef = useRef(false);
  const generation = useRef(0);
  const refreshing = useRef(0);

  const refresh = useCallback(async () => {
    refreshing.current += 1;
    const request = ++generation.current;
    try {
      const [nextStatus, nextResources, nextSync] = await Promise.allSettled([
        service.getStatus(),
        service.getResources(),
        service.getResourceSync(),
      ]);
      if (request !== generation.current) return;
      if (nextStatus.status === 'fulfilled') {
        const resourceValues =
          nextResources.status === 'fulfilled' ? nextResources.value : [];
        setStatus({
          ...nextStatus.value,
          resources_total: resourceValues.length,
          resources_available: resourceValues.filter(
            (resource) => resource.availability === 'reachable',
          ).length,
        });
        setError(null);
      } else {
        setStatus(null);
        setError(errorMessage(nextStatus.reason));
      }
      setResources(
        nextResources.status === 'fulfilled' ? nextResources.value : [],
      );
      setSync(nextSync.status === 'fulfilled' ? nextSync.value : null);
      if (nextResources.status === 'rejected')
        setNotice(errorMessage(nextResources.reason));
    } catch (cause) {
      if (request !== generation.current) return;
      // 旧的“已连接”状态在状态查询失败后不再可信。
      setStatus(null);
      setResources([]);
      setError(errorMessage(cause));
    } finally {
      refreshing.current -= 1;
    }
  }, [service]);

  useEffect(() => {
    // refresh 在 Native/演示服务返回后才更新状态，不会在 effect 中同步 setState。
    // eslint-disable-next-line react-hooks/set-state-in-effect
    void refresh();
    const timer = window.setInterval(() => {
      if (!busyRef.current && refreshing.current === 0) void refresh();
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
    sync,
    report,
    busy,
    notice,
    error,
    dismissNotice: () => setNotice(null),
    refresh,
    toggleConnection: () =>
      perform('connection', async () => {
        if (coreIsActive(status)) await service.disconnect();
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
