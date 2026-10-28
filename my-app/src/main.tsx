import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import ScreenshotWindow from './ScreenshotWindow';
import './index.css';

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    {new URLSearchParams(window.location.search).has('capture')
      ? <ScreenshotWindow sessionId={new URLSearchParams(window.location.search).get('capture') || ''} />
      : <App />}
  </React.StrictMode>,
);
