'use client';
// Speaker names: a per-viewer convenience, so localStorage is enough. Keyed by Deepgram's
// speaker number, which is only stable within one run of the bridge.
import { useEffect, useState } from 'react';

const KEY = 'speakerNames';

export const fallbackName = (speaker: number | null) => (speaker == null ? 'Unknown speaker' : `Speaker ${speaker + 1}`);

export function useNames() {
  const [names, setNames] = useState<Record<string, string>>({});
  useEffect(() => {
    try { setNames(JSON.parse(localStorage.getItem(KEY) ?? '{}') || {}); } catch { /* storage blocked */ }
  }, []);
  const rename = (key: string, name: string) => {
    const next = { ...names, [key]: name };
    setNames(next);
    try { localStorage.setItem(KEY, JSON.stringify(next)); } catch { /* storage blocked */ }
  };
  const name = (speaker: number | null) => names[String(speaker ?? 'unknown')]?.trim() || fallbackName(speaker);
  return { names, rename, name };
}
