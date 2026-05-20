import { listPullList } from '$lib/api/pull';

export const load = async () => {
  return { entries: await listPullList() };
};
