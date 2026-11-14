import { byteSize, MAX_MEMORY_BYTES, MAX_NOTE_BYTES } from './limits.ts';
import type { Saved } from './store.ts';

export const STORAGE_KEY = 'decision-maker:v1';
export const SAVE_ERROR = 'History is not being saved: browser storage is full or unavailable. Export or clear older history.';

export function readSaved(storage: Pick<Storage, 'getItem'>): Partial<Saved> {
  const raw = storage.getItem(STORAGE_KEY) ?? '{}';
  if (raw.length > 2 * 1024 * 1024) throw new Error('Saved data exceeds the loading limit. Export or clear browser storage before resuming history saves.');
  const value = JSON.parse(raw);
  if (value?.memory && (Object.keys(value.memory).length > 256 || byteSize(value.memory) > MAX_MEMORY_BYTES
      || Object.values(value.memory).some((m) => byteSize(m) > MAX_NOTE_BYTES))) {
    throw new Error('Saved memory exceeds its byte limit; the original saved copy has been preserved.');
  }
  return value && typeof value === 'object' ? value : {};
}

export function writeSaved(storage: Pick<Storage, 'setItem'>, saved: Partial<Saved>): string | null {
  try {
    storage.setItem(STORAGE_KEY, JSON.stringify(saved));
    return null;
  } catch {
    return SAVE_ERROR;
  }
}
