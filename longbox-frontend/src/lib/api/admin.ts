// Operations endpoints. POST /api/admin/restart responds 202 and the
// process exits ~500ms later — the caller should not expect any other
// signal, just wait long enough for Docker to bring the container back
// up before reloading the page.
import { apiFetch } from './client';

export function restartServer(): Promise<void> {
  return apiFetch('/admin/restart', { method: 'POST' });
}
