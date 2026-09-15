import Database from "@tauri-apps/plugin-sql";
import { listen } from "@tauri-apps/api/event";
import { isTauri } from "./runtime";

export type HistoryEntry = {
  id: number;
  text: string;
  provider: string;
  model: string;
  durationMs: number;
  createdAt: number;
};

type TranscriptEvent = {
  text: string;
  provider: string;
  model: string;
  duration_ms: number;
};

let dbPromise: Promise<Database> | null = null;

async function db(): Promise<Database | null> {
  if (!isTauri()) return null;
  if (!dbPromise) {
    dbPromise = (async () => {
      const d = await Database.load("sqlite:vibeking.db");
      await d.execute(`
        CREATE TABLE IF NOT EXISTS history (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          text TEXT NOT NULL,
          provider TEXT NOT NULL,
          model TEXT NOT NULL,
          duration_ms INTEGER NOT NULL,
          created_at INTEGER NOT NULL
        );
      `);
      await d.execute(
        `CREATE INDEX IF NOT EXISTS idx_history_created_at ON history (created_at DESC);`,
      );
      return d;
    })();
  }
  return dbPromise;
}

export async function addEntry(entry: Omit<HistoryEntry, "id" | "createdAt">) {
  const d = await db();
  if (!d) return;
  await d.execute(
    `INSERT INTO history (text, provider, model, duration_ms, created_at) VALUES ($1, $2, $3, $4, $5)`,
    [entry.text, entry.provider, entry.model, entry.durationMs, Date.now()],
  );
}

type Row = {
  id: number;
  text: string;
  provider: string;
  model: string;
  duration_ms: number;
  created_at: number;
};

export async function listEntries(limit = 200): Promise<HistoryEntry[]> {
  const d = await db();
  if (!d) return [];
  const rows = await d.select<Row[]>(
    `SELECT id, text, provider, model, duration_ms, created_at FROM history ORDER BY created_at DESC LIMIT $1`,
    [limit],
  );
  return rows.map((r) => ({
    id: r.id,
    text: r.text,
    provider: r.provider,
    model: r.model,
    durationMs: r.duration_ms,
    createdAt: r.created_at,
  }));
}

export type HistoryStats = { todayChars: number; totalChars: number };

export async function getStats(): Promise<HistoryStats> {
  const d = await db();
  if (!d) return { todayChars: 0, totalChars: 0 };
  const startOfDay = new Date();
  startOfDay.setHours(0, 0, 0, 0);
  const rows = await d.select<
    { total: number | null; today: number | null }[]
  >(
    `SELECT
       COALESCE(SUM(LENGTH(text)), 0) AS total,
       COALESCE(SUM(CASE WHEN created_at >= $1 THEN LENGTH(text) ELSE 0 END), 0) AS today
     FROM history`,
    [startOfDay.getTime()],
  );
  const row = rows[0] ?? { total: 0, today: 0 };
  return {
    todayChars: Number(row.today ?? 0),
    totalChars: Number(row.total ?? 0),
  };
}

export async function clearAll(): Promise<void> {
  const d = await db();
  if (!d) return;
  await d.execute(`DELETE FROM history`);
}

let unlistenTranscript: (() => void) | null = null;
let started = false;

export function startHistoryListener() {
  if (started) return;
  if (!isTauri()) return;
  started = true;
  void listen<TranscriptEvent>("transcript:complete", async (e) => {
    if (!e.payload?.text) return;
    await addEntry({
      text: e.payload.text,
      provider: e.payload.provider,
      model: e.payload.model,
      durationMs: e.payload.duration_ms,
    });
    window.dispatchEvent(new Event("vibeking:history-updated"));
  })
    .then((off) => (unlistenTranscript = off))
    .catch((err) =>
      console.warn("[vibeking] history listener failed:", err),
    );
}

export function stopHistoryListener() {
  unlistenTranscript?.();
  unlistenTranscript = null;
  started = false;
}
