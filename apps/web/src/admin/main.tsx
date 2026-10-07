import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { AdminRoot } from './AdminRoot';
import { createTauriHostBridge, isOperatorShell } from '../desktop-bridge/tauri-bridge';
import '../styles/app.css';

const container = document.getElementById('root');
if (!container) {
  throw new Error('Missing #root container');
}

// The bridge is Tauri IPC. Outside the operator shell (e.g. opened in a plain browser) there is no
// host to talk to, so we pass none and the UI explains that the desktop app is required. We never
// fabricate a bridge, because that would imply a host connection that does not exist.
const bridge = isOperatorShell() ? createTauriHostBridge() : undefined;

createRoot(container).render(
  <StrictMode>
    <AdminRoot bridge={bridge} />
  </StrictMode>,
);
