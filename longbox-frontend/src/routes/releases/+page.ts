import { redirect } from '@sveltejs/kit';

/** The `Releases` nav item lands here; it has no UI of its own, just
 *  bounces the user to the calendar (the natural default child).
 *  Preserves the parent path as a real route so deep links to
 *  `/releases` from older docs / bookmarks don't 404. */
export function load(): never {
  redirect(307, '/releases/calendar');
}
