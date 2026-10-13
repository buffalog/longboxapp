import { getPullFailures } from '$lib/api/needs_attention';
import { getRefreshStatus, listPullList } from '$lib/api/pull';

export const load = async () => {
  const [entries, pullFailures, refreshStatus] = await Promise.all([
    listPullList(),
    getPullFailures(),
    getRefreshStatus()
  ]);
  return { entries, pullFailureCount: pullFailures.length, refreshStatus };
};
