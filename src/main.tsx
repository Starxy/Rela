import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import App from './App';
import { createService } from './services/rela';
import { errorMessage } from './types';
import './styles.css';

const root = createRoot(document.getElementById('root')!);
createService()
  .then((service) => {
    root.render(
      <StrictMode>
        <App service={service} />
      </StrictMode>,
    );
  })
  .catch((error: unknown) => {
    root.render(
      <main className="startup-error">
        <img src="/rela.svg" alt="Rela" width="56" />
        <h1>欢迎使用 Rela</h1>
        <p role="alert">{errorMessage(error)}</p>
      </main>,
    );
  });
