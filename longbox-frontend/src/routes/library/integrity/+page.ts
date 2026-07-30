import { getAnalyzeStatus, getFindings } from '$lib/api/integrity';

export const load = async () => {
  // Both in parallel: the page needs the findings to render anything and the
  // analyze status to say whether the content-duplicate count is a total or
  // a floor.
  const [findings, analyze] = await Promise.all([getFindings(), getAnalyzeStatus()]);
  return { findings, analyze };
};
