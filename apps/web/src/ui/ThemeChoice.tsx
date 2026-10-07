import { useState } from 'react';
import { Button } from './Button';
export function ThemeChoice() {
  const [light, setLight] = useState(() => document.documentElement.dataset.theme === 'light');
  return <Button onClick={() => { const next = !light; setLight(next); document.documentElement.dataset.theme = next ? 'light' : 'dark'; }} aria-pressed={light}>{light ? 'Dark theme' : 'Light theme'}</Button>;
}
