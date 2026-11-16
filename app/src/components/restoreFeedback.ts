// Shared restore-feedback: the UI enqueues a restore job (a row in
// restore_requests); the extension picks it up within ~30 s (and sooner on
// events). Without feedback, a dead browser or a failing URL looks identical
// to success — so poll recent_restore_requests for the outcome and say so.

import { api } from '../api';

export async function notifyRestoreResult(
  notify: (msg: string) => void,
  jobPromise: Promise<number>,
): Promise<void> {
  let jobId: number;
  try {
    jobId = await jobPromise;
  } catch (e) {
    notify(`Restore failed: ${String(e)}`);
    return;
  }
  notify('Restoring…');
  const deadline = Date.now() + 8000;
  while (Date.now() < deadline) {
    await new Promise((r) => setTimeout(r, 500));
    try {
      const jobs = await api.recentRestoreRequests();
      const job = jobs.find((j) => j[0] === jobId);
      if (!job) return;
      const [, , status, error] = job;
      if (status === 'done') {
        notify('Restored.');
        return;
      }
      if (status === 'failed') {
        notify(error ? `Restore failed: ${error}` : 'Restore failed.');
        return;
      }
    } catch {
      // Transient read failure — keep polling until the deadline.
    }
  }
  notify('Restore dispatched. If the browser is open, the tabs open within a moment; if it is closed, they will open on next launch.');
}
