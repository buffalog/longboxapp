import { getReleaseCalendar } from '$lib/api/releases';

function pad(n: number): string {
  return String(n).padStart(2, '0');
}

function iso(d: Date): string {
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

/** The current comics shipping week — Wednesday through the following
 *  Tuesday. New comics ship Wednesday; `from` is the most-recent
 *  Wednesday at or before today, `to` is six days later. On a Friday
 *  load this is Wed-of-this-week → Tue-of-next-week, no drift. */
function shippingWeek(): { from: string; to: string } {
  const today = new Date();
  // getDay(): 0=Sun .. 6=Sat; Wednesday is 3.
  const back = (today.getDay() - 3 + 7) % 7;
  const from = new Date(today.getFullYear(), today.getMonth(), today.getDate() - back);
  const to = new Date(from.getFullYear(), from.getMonth(), from.getDate() + 6);
  return { from: iso(from), to: iso(to) };
}

export const load = async ({ url }) => {
  const week = shippingWeek();
  const from = url.searchParams.get('from') ?? week.from;
  const to = url.searchParams.get('to') ?? week.to;
  return { from, to, rows: await getReleaseCalendar(from, to) };
};
