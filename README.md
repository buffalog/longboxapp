# LongBox

## What is this?

LongBox is a self-hosted comic library catalog. You point it at a folder full of CBZ, CBR, or CB7 files, and it figures out what you have. It parses filenames, matches them against ComicVine metadata, pulls down covers and issue details, and gives you a clean dashboard showing what you own, what you're missing, and what needs your attention. If you use Usenet, it connects to SABnzbd and Newznab indexers to search for and download the issues you don't have. It runs as a single Docker container with a SQLite database that lives on your machine, not in someone else's cloud.

## Why does this exist?

Because of Mylar.

Mylar was the gold standard for comic library management. EvilHero built and maintained that project for years, quietly and consistently, giving the comic collecting community a tool that nothing else came close to matching. That kind of sustained, unglamorous, one-person open source work doesn't get the recognition it deserves. If you ever used Mylar to manage your library, you owe that person a debt of gratitude, and so do I.

When Mylar's development slowed, a lot of us were left without a tool that did what it did. I looked around for alternatives and didn't find anything that fit. So I started building one. LongBox isn't a fork of Mylar and it doesn't share any code. It's a ground-up rewrite in a completely different stack, informed by the years I spent using Mylar and understanding what it got right. The organizational model, the ComicVine integration, the Usenet workflow: Mylar established the patterns that this project builds on. That lineage matters, and I want to be transparent about it.

## Who built this?

One person with an M5 Max, Claude Code, and a mass of coffee. I'm a solutions engineer by trade, not a professional software developer, and this is my first Rust project. LongBox started as a way to manage my own collection of about 5,500 comics, and it grew into something I thought other collectors might find useful. It is not backed by a company. There is no business model. There are no analytics, no telemetry, no tracking. It's a tool for people who collect comics and want to keep their library organized on their own hardware.

## How does it work?

You set up a Docker container, give it your ComicVine API key and the path to your comic library, and run it. LongBox scans your library, discovers every comic file it can find, parses the filenames to extract series names and issue numbers, and attempts to match each file against ComicVine's database. When it finds a match, it pulls down metadata: series title, publisher, cover art, cover dates, issue counts. All of that shows up in the web UI at localhost:3000.

From there you can see your entire collection at a glance. The dashboard breaks everything down: how many series you're tracking, how many issues exist across those series, how many you own, how many need review (files that matched with lower confidence), and how many are missing. You can drill into any series to see issue-by-issue status, fix incorrect matches, add new series by searching ComicVine, and manage a pull list for series you're actively following.

If you connect SABnzbd and one or more Newznab indexers, LongBox can search for missing issues automatically. It queries your indexers, filters out reprints, collections, and foreign editions, submits NZBs to SABnzbd, and when the downloads complete, it matches and places the files into the correct series folder in your library. It rejects corrupt or partial downloads (anything under a configurable size floor) and cleans up empty download folders. Adding a new series triggers an immediate search for all missing issues, so the whole workflow from "I want this series" to "it's downloading" is one click.

There's a release calendar that shows what's on sale this week across all publishers, a pull list that tracks new issues for series you follow, and a needs-attention page that surfaces anything the system couldn't handle on its own. Every threshold, filter, and keyword is editable from the settings page without restarting the container.

## What's it built with?

The backend is Rust using Actix-web and SQLx, with a SQLite database. The frontend is SvelteKit. Metadata comes from ComicVine and Metron. Download integration is SABnzbd with Newznab-compatible indexers. The whole thing ships as a single Docker image with no external services or dependencies.

## How do I set it up?

See the **[Setup Guide](SETUP.md)** for detailed, step-by-step instructions covering macOS, Linux, and Windows. The guide is written for users of all experience levels, including people who have never opened a terminal before.

The short version:

```bash
mkdir ~/longbox && cd ~/longbox
# Create docker-compose.yml and .env (see SETUP.md)
docker compose up -d
# Open http://localhost:3000
```

## What's next?

LongBox is actively developed. I use it every day to manage my own library, which means bugs get found and fixed in real time. Feature requests and bug reports are welcome via [Issues](https://github.com/buffalog/longboxapp/issues).

## Acknowledgments

To EvilHero: thank you. Mylar gave this community something real, and the work you put into it over all those years mattered. LongBox exists because Mylar showed what was possible.

Thanks also to the people behind [ComicVine](https://comicvine.gamespot.com/), [Metron](https://metron.cloud/), [SABnzbd](https://sabnzbd.org/), and the Newznab ecosystem. None of this works without the tools and data those teams maintain.

---

## Screenshots

*Coming soon.*

<!--
![Dashboard](docs/screenshots/dashboard.png)
![Series Detail](docs/screenshots/series-detail.png)
![Missing](docs/screenshots/missing.png)
![Settings](docs/screenshots/settings.png)
-->

---

## License

*License TBD. See [LICENSE](LICENSE) when published.*
