import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import '../design/global.css';
import './main.css';
import { App } from './App';

const root = document.querySelector('#root');
if (root === null) throw new Error('Renderer root is missing');

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
