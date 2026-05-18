import { getSettings } from '$lib/api/settings';

export const load = async () => {
  const settings = await getSettings();
  return { settings };
};
