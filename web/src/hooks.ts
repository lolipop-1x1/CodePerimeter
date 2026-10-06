import { useEffect, useRef, useState } from 'react';
import { api, consoleApi } from './api';

export function usePolling<T>(path: string, body: unknown, active = true, interval = 500) {
  const [data, setData] = useState<T>();
  const [error, setError] = useState<string>();
  const [loading, setLoading] = useState(true);
  const [revision, setRevision] = useState(0);
  const [updatedAt, setUpdatedAt] = useState<number>();
  const serialized = JSON.stringify(body);
  const lastBody = useRef('');
  useEffect(() => {
    if (!active) return;
    let alive = true;
    let timer: ReturnType<typeof setTimeout>;
    let controller: AbortController;
    if (lastBody.current !== path + serialized) {
      setData(undefined);
      setLoading(true);
      lastBody.current = path + serialized;
    }
    const load = async (initial = false) => {
      // 通知可能打开后台标签页，首次加载不能因可见性变化而永久停在骨架屏。
      if (document.hidden && !initial) return;
      controller = new AbortController();
      try {
        const result = path === '/api/console'
          ? await consoleApi<T>((body as { action: string }).action, (body as { payload?: unknown }).payload, controller.signal)
          : await api<T>(path, body, controller.signal);
        if (alive) { setData(result); setError(undefined); setUpdatedAt(Date.now()); }
      } catch (issue) {
        if (alive && !(issue instanceof DOMException && issue.name === 'AbortError')) setError(issue instanceof Error ? issue.message : '数据加载失败。');
      } finally {
        if (alive) { setLoading(false); if (interval > 0) timer = setTimeout(load, interval); }
      }
    };
    const visibility = () => { if (!document.hidden) { clearTimeout(timer); controller?.abort(); void load(); } };
    void load(true);
    document.addEventListener('visibilitychange', visibility);
    return () => { alive = false; clearTimeout(timer); controller?.abort(); document.removeEventListener('visibilitychange', visibility); };
  }, [path, serialized, active, interval, revision]);
  return { data, error, loading, updatedAt, refresh: () => setRevision(value => value + 1) };
}

export function useAction() {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [success, setSuccess] = useState<string>();
  async function run<T>(action: () => Promise<T>, message?: string): Promise<T | undefined> {
    if (busy) return;
    setBusy(true); setError(undefined); setSuccess(undefined);
    try { const result = await action(); if (message) setSuccess(message); return result; }
    catch (issue) { setError(issue instanceof Error ? issue.message : '操作失败。'); return undefined; }
    finally { setBusy(false); }
  }
  return { busy, error, success, run, reset: () => { setError(undefined); setSuccess(undefined); } };
}
