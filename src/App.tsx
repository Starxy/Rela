import { Settings2, X } from 'lucide-react';
import { useState } from 'react';
import { ConnectionDetails } from './components/ConnectionDetails';
import { Diagnostics } from './components/Diagnostics';
import { Modal } from './components/Modal';
import { useRela } from './hooks/useRela';
import { Home } from './pages/Home';
import { Resources } from './pages/Resources';
import { Settings } from './pages/Settings';
import type { RelaService } from './types';

const modalTitles = {
  settings: '设置',
  resources: '实验室资源',
  diagnostics: '连接诊断',
  details: '连接详情',
};
type ModalView = keyof typeof modalTitles;

export default function App({ service }: { service: RelaService }) {
  const [modal, setModal] = useState<ModalView | null>(null);
  const rela = useRela(service);

  const showDiagnostics = () => {
    setModal('diagnostics');
    void rela.diagnose();
  };

  return (
    <div className="app-window">
      <header className="app-header">
        <div className="brand">
          <img src="/rela.svg" alt="" width="26" height="26" />
          <h1>Rela</h1>
        </div>
        <button
          className="icon-button"
          onClick={() => setModal('settings')}
          aria-label="设置"
          title="设置"
        >
          <Settings2 size={19} aria-hidden="true" />
        </button>
      </header>
      <Home
        status={rela.status}
        busy={rela.busy}
        error={rela.error}
        onConnect={() => void rela.toggleConnection()}
        onDiagnose={showDiagnostics}
        onResources={() => setModal('resources')}
        onDetails={() => setModal('details')}
      />
      {modal && (
        <Modal title={modalTitles[modal]} onClose={() => setModal(null)}>
          {modal === 'settings' && (
            <Settings service={service} onSaved={() => setModal(null)} />
          )}
          {modal === 'resources' && (
            <Resources
              resources={rela.resources}
              busy={!!rela.busy}
              onOpen={(id) => void rela.openResource(id)}
            />
          )}
          {modal === 'details' && (
            <ConnectionDetails status={rela.status} error={rela.error} />
          )}
          {modal === 'diagnostics' && (
            <Diagnostics
              report={rela.report}
              busy={rela.busy}
              onRun={() => void rela.diagnose()}
              onExport={() => void rela.exportLogs()}
            />
          )}
          {rela.notice && (
            <div className="inline-notice" role="status">
              <span>{rela.notice}</span>
              <button
                className="icon-button"
                onClick={rela.dismissNotice}
                aria-label="关闭提示"
              >
                <X size={15} aria-hidden="true" />
              </button>
            </div>
          )}
        </Modal>
      )}
      {!modal && rela.notice && (
        <div className="toast" role="status">
          <span>{rela.notice}</span>
          <button
            className="icon-button"
            onClick={rela.dismissNotice}
            aria-label="关闭提示"
          >
            <X size={16} aria-hidden="true" />
          </button>
        </div>
      )}
    </div>
  );
}
