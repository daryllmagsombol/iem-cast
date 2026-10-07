import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { createReceiverController } from '../transport/ReceiverController';
import type { ReceiverController } from '../receiver-ports';
import { MusicianRoot } from './MusicianRoot';
import { createBrowserReceiverPorts } from './ports';
import '../styles/app.css';

const container = document.getElementById('root');
if (!container) {
  throw new Error('Missing #root container');
}

/**
 * Compose the real browser receiver. Any missing browser primitive leaves `controller` null so the
 * root renders the honest unavailable state rather than an inert fake controller.
 */
let controller: ReceiverController | undefined;
try {
  controller = createReceiverController(createBrowserReceiverPorts());
} catch {
  controller = undefined;
}

createRoot(container).render(
  <StrictMode>
    <MusicianRoot controller={controller} />
  </StrictMode>,
);
