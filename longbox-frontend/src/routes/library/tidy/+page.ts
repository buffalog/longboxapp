import { listPhantoms, listUntracked } from '$lib/api/reconcile';

export const load = async () => {
  const [phantoms, untracked] = await Promise.all([listPhantoms(), listUntracked()]);
  return { phantoms, untracked };
};
