import { getDownloader } from '$lib/api/downloader';
import { listIndexers } from '$lib/api/indexers';
import { getOpds } from '$lib/api/opds';
import { listFilters } from '$lib/api/publishers';
import { getSettings } from '$lib/api/settings';
import { listWebhooks } from '$lib/api/webhooks';

export const load = async () => {
  const [settings, publisherFilters, indexers, downloader, webhooks, opds] = await Promise.all([
    getSettings(),
    listFilters(),
    listIndexers(),
    getDownloader(),
    listWebhooks(),
    getOpds()
  ]);
  return { settings, publisherFilters, indexers, downloader, webhooks, opds };
};
