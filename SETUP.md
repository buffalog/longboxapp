# LongBox Setup Guide

A step-by-step guide to self-hosting LongBox, the comic library catalog. Written for users of all experience levels, covering macOS, Linux, and Windows.

---

## What LongBox does

LongBox tracks your comic library. Point it at a folder of CBZ/CBR files, and it will scan, identify, and catalog every issue against ComicVine metadata. It surfaces what you own, what you're missing, and (optionally) integrates with Usenet tools to fill the gaps automatically.

## What you'll need

- **Docker** (required, Docker Desktop 4.x+ or Docker Engine 20.10+)
- **A ComicVine API key** (required, free)
- **A folder of comic files** (CBZ, CBR, or CB7)
- **SABnzbd + a Newznab indexer** (optional, for automated downloading)

**Supported architectures:** LongBox publishes multi-architecture Docker images for both **arm64** (Apple Silicon, Raspberry Pi 4/5) and **amd64** (Intel/AMD). Docker will pull the correct image automatically.

---

## Step 1: Open your terminal

Every step in this guide uses a terminal (a text window where you type commands). If you've never used one, don't worry. You'll be copying and pasting commands, not memorizing them.

### macOS

Press **Cmd + Space** to open Spotlight, type **Terminal**, and press Enter. A window with a text prompt will appear. This is your terminal.

Tip: You can also find Terminal in Applications > Utilities > Terminal.

### Linux

On most Linux desktops, press **Ctrl + Alt + T** to open a terminal. On Ubuntu, you can also search for "Terminal" in the application menu.

### Windows

Press the **Windows key**, type **PowerShell**, and click "Windows PowerShell." A blue text window will appear. This is your terminal.

Do not use the older "Command Prompt" (cmd.exe). PowerShell is required for the commands in this guide.

---

## Step 2: Install Docker

Docker is the tool that runs LongBox. Think of it as a lightweight virtual machine that packages the app and everything it needs into one container. LongBox requires **Docker Compose v2** (the `docker compose` command with a space, not the older hyphenated `docker-compose`).

### macOS

First, check whether your Mac uses Apple Silicon or Intel. Click the Apple menu () in the top-left corner, then click **About This Mac**. Look for the "Chip" line. If it says "Apple M1," "Apple M2," "Apple M3," or similar, you have Apple Silicon. If it says "Intel," you have Intel.

1. Go to [https://www.docker.com/products/docker-desktop/](https://www.docker.com/products/docker-desktop/) and download the version that matches your chip.
2. Open the downloaded `.dmg` file and drag Docker to your Applications folder.
3. Open Docker from your Applications folder. It will ask for permission to install a helper; allow it.
4. Wait for the Docker icon in the menu bar (top of screen, near the clock) to stop animating. When it's still, Docker is ready.
5. Go back to your terminal and type these two commands, pressing Enter after each:

```
docker --version
docker compose version
```

You should see version numbers printed. If you see "command not found," Docker didn't install correctly. Try restarting your Mac and opening Docker Desktop again.

### Linux (Ubuntu/Debian)

Copy and paste this entire block into your terminal and press Enter:

```bash
curl -fsSL https://get.docker.com | sh
sudo usermod -aG docker $USER
```

The first line installs Docker. The second line lets you run Docker without typing `sudo` every time. After running these commands, **log out and log back in** (or restart your computer) for the change to take effect.

Then verify it worked:

```bash
docker --version
docker compose version
```

For distributions other than Ubuntu/Debian, follow the [official Docker Engine install docs](https://docs.docker.com/engine/install/).

### Windows

1. Go to [https://www.docker.com/products/docker-desktop/](https://www.docker.com/products/docker-desktop/) and download Docker Desktop for Windows.
2. Run the installer. When it asks about WSL 2, **say yes** (this is the recommended option).
3. Restart your computer when prompted.
4. After restarting, open Docker Desktop from the Start menu. It may take a minute to finish setting up WSL 2 on first launch.
5. Open PowerShell and verify:

```
docker --version
docker compose version
```

**Windows note:** Docker Desktop requires WSL 2 or Hyper-V. Windows Home users must use WSL 2 (it works well). Windows Pro/Enterprise can use either. If you see errors about virtualization, you may need to enable it in your BIOS. Search "enable virtualization [your computer brand]" for instructions.

---

## Step 3: Get a ComicVine API key

LongBox uses ComicVine for series and issue metadata (titles, covers, cover dates, publishers).

1. Go to [https://comicvine.gamespot.com/api/](https://comicvine.gamespot.com/api/).
2. Create a free account or sign in.
3. Your API key will be displayed on the API page. It's a long string of letters and numbers. Copy it and save it somewhere (a text file, a note, or your clipboard). You'll need it in a few steps.

ComicVine's free tier is rate-limited. LongBox tracks its remaining budget (visible in the top-right corner of the UI as "CV X/180") and throttles itself automatically.

---

## Step 4: Find your library path

LongBox needs to know where your comic files live on your computer. This is called an "absolute path," which is the full address of a folder starting from the root of your file system.

### macOS

Open Finder and navigate to your comics folder. Right-click the folder, hold the **Option** key, and click **Copy "[folder name]" as Pathname**. This copies the full path to your clipboard.

It will look something like: `/Users/yourname/Comics`

### Linux

Open your file manager and navigate to your comics folder. The address bar usually shows the path. If not, right-click the folder and look for "Properties." The path will look something like: `/home/yourname/comics`

You can also find it from the terminal: navigate into the folder with `cd` and then type `pwd` to print the full path.

### Windows

Open File Explorer and navigate to your comics folder. Click in the address bar at the top. It will change from a breadcrumb view to show the full path. Copy it. It will look something like: `C:\Users\yourname\Comics`

**Important for Windows:** When you use this path in the steps below, convert the backslashes to forward slashes. So `C:\Users\yourname\Comics` becomes `C:/Users/yourname/Comics`.

---

## Step 5: Create your project directory

This is a folder for LongBox's configuration files. It is NOT your comic library. It's just a small folder that holds two config files.

Copy and paste the following into your terminal:

### macOS / Linux

```bash
mkdir -p ~/longbox
cd ~/longbox
```

### Windows (PowerShell)

```powershell
mkdir $HOME\longbox -Force
cd $HOME\longbox
```

---

## Step 6: Create the configuration files

You need two files in your project directory: `docker-compose.yml` and `.env`. The easiest way to create them is by pasting commands into your terminal. Do NOT try to create these with TextEdit, Notepad, or Word. Those editors can add invisible formatting that breaks the files.

### Create docker-compose.yml

Copy this entire command and paste it into your terminal. Press Enter.

**macOS / Linux:**

```bash
cat > docker-compose.yml << 'EOF'
services:
  longbox:
    image: longboxapp/longbox:latest
    container_name: longbox
    restart: unless-stopped
    ports:
      - "3000:3000"
      # NOTE: This binds to all interfaces, making LongBox accessible
      # from other devices on your network. To restrict to local-only
      # access, change to "127.0.0.1:3000:3000".
    volumes:
      - ${DB_PATH}:/data
      - ${LIBRARY_PATH}:/library
      - ${DOWNLOAD_WATCH_HOST:-./.empty-watch}:/watch
    environment:
      LIBRARY_ROOT_PATH: /library
      DATABASE_URL: "sqlite:/data/longbox.db?mode=rwc"
      BIND_ADDR: "0.0.0.0:3000"
      LOG_LEVEL: ${LOG_LEVEL:-info}
      MATCH_THRESHOLD: ${MATCH_THRESHOLD:-0.75}
      COMICVINE_API_KEY: ${COMICVINE_API_KEY}
      METRON_API_USER: ${METRON_API_USER:-}
      METRON_API_PASSWORD: ${METRON_API_PASSWORD:-}
      DOWNLOAD_WATCH_PATH: ${DOWNLOAD_WATCH_PATH:-}
      HOST_LIBRARY_PATH: ${HOST_LIBRARY_PATH:-}
      OPDS_BASE_URL: ${OPDS_BASE_URL:-}
    healthcheck:
      test: ["CMD", "wget", "-q", "--spider", "http://127.0.0.1:3000/api/health"]
      interval: 30s
      timeout: 5s
      retries: 3
      start_period: 10s
    # Uncomment the next line on Linux if SABnzbd can't connect
    # using host.docker.internal as the base URL:
    # extra_hosts: ["host.docker.internal:host-gateway"]
EOF
```

**Windows (PowerShell):**

On Windows, download the file directly from the repository instead:

```powershell
Invoke-WebRequest -Uri "https://raw.githubusercontent.com/longbox-app/longbox/main/docker-compose.example.yml" -OutFile "docker-compose.yml"
```

If the download doesn't work (the repository may not be public yet), create the file manually: open VS Code or Notepad++, paste the YAML content from the macOS/Linux section above (everything between the two `EOF` lines), and save it as `docker-compose.yml` in your project folder. Make sure your editor saves it as a plain text file, not `.txt`.

### Create .env

This file holds your personal settings. Replace the placeholder values with your actual paths and API key.

**macOS / Linux:**

```bash
cat > .env << 'EOF'
# REQUIRED: Your ComicVine API key (the long string from Step 3)
COMICVINE_API_KEY=paste_your_key_here

# REQUIRED: Absolute path to your comic library on the host.
# NOTE: LongBox writes to this folder when placing downloaded files
# into series directories. It will not modify or delete your existing files.
LIBRARY_PATH=/paste/your/library/path/here

# REQUIRED: Where to store the LongBox database on the host.
# The path must be a DIRECTORY, not a file.
DB_PATH=/paste/your/library/path/here/.longbox/db

# OPTIONAL: Host-side library path for "Show in Finder/Explorer."
# Usually the same as LIBRARY_PATH.
HOST_LIBRARY_PATH=/paste/your/library/path/here

# OPTIONAL: SABnzbd completed downloads folder for post-processing.
# Set both to enable automated file placement.
# When these are not set, Docker creates a harmless .empty-watch
# directory in your project folder as a placeholder. You can ignore it.
# DOWNLOAD_WATCH_HOST=/path/to/sabnzbd/complete
# DOWNLOAD_WATCH_PATH=/watch

# OPTIONAL: Metron credentials for the release calendar.
# METRON_API_USER=
# METRON_API_PASSWORD=

# OPTIONAL: Public URL reader devices use to reach the OPDS catalog.
# Must be reachable from the device (a LAN IP, Tailscale host, or domain),
# not localhost. See "Reading on devices with OPDS" below.
# OPDS_BASE_URL=http://192.168.1.50:3000

# OPTIONAL: Logging level (info, debug, warn, error)
# LOG_LEVEL=info
EOF
```

**Windows (PowerShell):**

```powershell
@"
COMICVINE_API_KEY=paste_your_key_here
LIBRARY_PATH=C:/paste/your/library/path/here
DB_PATH=C:/paste/your/library/path/here/.longbox/db
HOST_LIBRARY_PATH=C:/paste/your/library/path/here
"@ | Out-File -Encoding utf8 .env
```

### Now edit .env with your real values

Open the `.env` file in a text editor and replace the placeholder values:

**macOS:** `open -e .env` (opens in TextEdit)
**Linux:** `nano .env` (press Ctrl+O to save, Ctrl+X to exit)
**Windows:** `notepad .env`

Replace:
- `paste_your_key_here` with the API key you copied in Step 3
- `/paste/your/library/path/here` (or `C:/paste/your/library/path/here`) with the path you found in Step 4

### Example .env values by platform

**macOS:**
```
COMICVINE_API_KEY=a1b2c3d4e5f6g7h8i9j0
LIBRARY_PATH=/Users/sam/Comics
DB_PATH=/Users/sam/Comics/.longbox/db
HOST_LIBRARY_PATH=/Users/sam/Comics
```

**Linux:**
```
COMICVINE_API_KEY=a1b2c3d4e5f6g7h8i9j0
LIBRARY_PATH=/home/sam/comics
DB_PATH=/home/sam/comics/.longbox/db
HOST_LIBRARY_PATH=/home/sam/comics
```

**Windows:**
```
COMICVINE_API_KEY=a1b2c3d4e5f6g7h8i9j0
LIBRARY_PATH=C:/Users/sam/Comics
DB_PATH=C:/Users/sam/Comics/.longbox/db
HOST_LIBRARY_PATH=C:/Users/sam/Comics
```

**macOS note:** If your library is on an external or network drive, you need to add that path to Docker Desktop's allow-list. Open Docker Desktop > Settings > Resources > File Sharing and add your library path. Without this, Docker silently ignores the folder and LongBox will report an empty library.

**Windows note:** If your library is on a drive other than C:, add it in Docker Desktop > Settings > Resources > File Sharing.

---

## Step 7: Create the database directory and start LongBox

The database directory must exist before you start the container.

### macOS / Linux

```bash
mkdir -p "$(grep DB_PATH .env | cut -d= -f2)"
docker compose up -d
```

### Windows (PowerShell)

```powershell
$dbPath = (Get-Content .env | Select-String "DB_PATH=").ToString().Split("=",2)[1]
New-Item -ItemType Directory -Path $dbPath -Force
docker compose up -d
```

This pulls the image from the registry and starts the container in the background. The first pull downloads about 100 MB.

### Verify it's running

```bash
docker ps
```

You should see `longbox` listed with status "Up" and port `0.0.0.0:3000->3000/tcp`.

Open your web browser and go to:

```
http://localhost:3000
```

You should see the LongBox dashboard.

### If something went wrong

**"no matching manifest" or "image not found":** The image hasn't been published yet, or the name is wrong. Check the project's GitHub page for the correct image name.

**"port is already allocated":** Another application is using port 3000. Open your `.env` file and add this line: `LONGBOX_PORT=3001`. Then change the ports line in docker-compose.yml from `"3000:3000"` to `"3001:3000"` and visit `http://localhost:3001` instead.

**Container starts but the page won't load:** Wait 15 seconds (LongBox takes a moment to initialize). If it still doesn't load, check the logs:

```bash
docker logs longbox
```

Common causes: missing or invalid COMICVINE_API_KEY, or the DB_PATH directory doesn't exist.

**"error during connect" or "Cannot connect to the Docker daemon":** Docker Desktop isn't running. Open Docker Desktop from your Applications (macOS), Start menu (Windows), or make sure the Docker service is started on Linux (`sudo systemctl start docker`).

---

## Step 8: Initial setup in LongBox

Everything from here is in the browser. No more terminal commands.

### Verify ComicVine

Your API key was passed via the environment variable, so ComicVine should already be configured. Go to the **Settings** page and verify you see "ComicVine API: Configured."

### Run your first scan

From the Dashboard, click **Scan library**. LongBox will walk your library folder, discover comic files, parse their filenames, and attempt to match them against ComicVine metadata.

The first scan takes time depending on library size. A library of 5,000 files takes roughly 5 to 10 minutes. Progress shows on the Dashboard.

### Add series

If your library is organized by series folders (e.g., `Saga (2012)/Saga 001.cbz`), the scanner will create series entries automatically. You can also manually add series from the **Add** page by searching ComicVine.

### Review matches

After the scan, check the Dashboard tiles:

- **Owned**: files successfully matched to ComicVine issues
- **Needs Review**: files matched with lower confidence (below threshold)
- **Unmatched**: files the scanner couldn't parse or match

Visit the **Needs Attention** page (under Library in the navigation) to review and resolve issues.

---

## Step 9: Optional setup

### Automated downloading with SABnzbd

If you use Usenet and want LongBox to automatically search for and download missing issues:

1. **SABnzbd**: Install and configure [SABnzbd](https://sabnzbd.org/) on your system. Note the API key (Settings > General > API Key) and the completed downloads folder.

2. **Newznab indexer**: You need at least one Newznab-compatible indexer (e.g., NZBGeek, NZBPlanet). Get an account and note the API URL and key.

3. **Update your `.env`** (open it in a text editor and uncomment/edit these lines):
   ```env
   DOWNLOAD_WATCH_HOST=/path/to/sabnzbd/complete
   DOWNLOAD_WATCH_PATH=/watch
   ```

4. **Restart the container**:
   ```bash
   docker compose down && docker compose up -d
   ```

5. **Configure in LongBox**: Go to Settings and configure:
   - **Downloader**: Add SABnzbd with its base URL and API key. The URL should be `http://host.docker.internal:8080` (replace 8080 with your SABnzbd port). Click "Test connection" to verify.
   - **Indexers**: Add your Newznab indexer(s) with their URL and API key. Click "Test" to verify.

6. **Search for missing issues**: Go to the Missing page and click "Search N missing." LongBox will query your indexers, submit NZBs to SABnzbd, and automatically move completed downloads into your library.

**Linux users:** If "Test connection" fails with SABnzbd, `host.docker.internal` may not work on your Docker version. Either use your machine's LAN IP instead (e.g., `http://192.168.1.100:8080`), or uncomment the `extra_hosts` line in your docker-compose.yml, then restart the container.

#### Optional: faster retries with the SAB post-processing script

LongBox's pull engine polls SABnzbd once per sweep (daily by default), so a failed download — par2 repair failure, incomplete article count, etc. — sits for up to 24h before LongBox notices and retries with a different release. You can close that gap by giving SABnzbd a one-liner script that pings LongBox at the moment a job ends.

1. Save this script as `longbox-notify.sh` in your SABnzbd scripts folder (Config → Folders → "Post-Processing Scripts Folder"):

   ```bash
   #!/bin/bash
   # LongBox download notification script. SABnzbd invokes this on
   # every job completion; LongBox ignores successes (Phase B catches
   # the file in the watch folder) and immediately retries failures
   # against a different release.
   #
   # SABnzbd positional args ($1..$8):
   #   $1 final dir, $2 original name, $3 clean name, $4 msgid,
   #   $5 category, $6 group, $7 status, $8 nzo_id

   NZO_ID="$8"
   STATUS="$7"

   curl -s -X POST http://localhost:8081/api/downloader/notify \
     -H "Content-Type: application/json" \
     -d "{\"nzo_id\": \"${NZO_ID}\", \"status\": \"${STATUS}\", \"fail_msg\": \"\"}" \
     > /dev/null

   exit 0
   ```

2. Make it executable: `chmod +x /path/to/longbox-notify.sh`.

3. In SABnzbd, set it on the category LongBox uses (Config → Categories → your category → "Script: longbox-notify.sh"), or as a global default if every job should ping LongBox.

Notes:
- The URL `http://localhost:8081` is the LongBox host port from the default `docker-compose.yml`. Match it to whatever host:port your deployment exposes.
- The endpoint always returns 200 — it never errors back to SAB, so a misconfigured URL just silently no-ops.
- "Completed" notifications are ignored. LongBox finds the file in the watch folder via Phase B regardless. Only failures trigger an immediate retry search.

### Metron release calendar

For the release calendar feature (new issues on sale this week), create a free account at [https://metron.cloud/](https://metron.cloud/) and add your credentials to `.env`:

```env
METRON_API_USER=your_username
METRON_API_PASSWORD=your_password
```

Then restart the container: `docker compose down && docker compose up -d`

### Reading on devices with OPDS

LongBox can serve your library to OPDS comic readers — apps like Chunky,
Panels, KyBook, or Cantook — so you can browse and download issues on a
phone or tablet.

1. In `.env`, set `OPDS_BASE_URL` to the address other devices reach
   LongBox at (a LAN IP, a Tailscale hostname, or a domain) including the
   port, then restart the container:

   ```env
   # The URL your reader devices use to reach LongBox. OPDS links are
   # absolute, so this must be reachable from the device — not localhost.
   OPDS_BASE_URL=http://192.168.1.50:3000
   ```

2. In the LongBox web UI, open **Settings → OPDS**, tick **Enabled**, set a
   **username** and **password**, and **Save**. An API token is generated
   on first save — some readers take a token instead of a password.

3. In your reader app, add a new OPDS/catalog source. Paste the **Catalog
   URL** shown in Settings (`{OPDS_BASE_URL}/opds/v1`) and enter the
   username and password. Covers and downloads stream straight from
   LongBox.

The catalog is disabled by default and returns `503` until you enable it
and set a username. Every request requires the credentials above — covers
are cached under `/data/covers` inside the existing database volume, so no
new mount is needed.

---

## Maintenance

### Updating LongBox

When a new version is available:

```bash
cd ~/longbox
docker compose pull
docker compose up -d
```

Docker Compose will automatically restart the container with the new image. If you have in-progress scans or downloads, they will be interrupted and will resume on the next scheduled cycle.

### Backups

LongBox stores everything in a single SQLite database. The database is stored on your computer at the path you set in `DB_PATH`. Back up this directory however you normally back up files (Time Machine, rsync, file copy, etc.).

LongBox also creates a snapshot of the database before any destructive operation (like deleting a series). These snapshots are saved in the same directory as the database.

### Viewing logs

If something seems wrong, check the logs:

```bash
docker logs longbox --tail 100
```

Follow logs in real time (press Ctrl+C to stop):

```bash
docker logs longbox -f
```

### Restarting

```bash
docker compose restart
```

Or from the LongBox UI: Settings > System > Restart LongBox.

### Stopping

```bash
docker compose down
```

Your data is safe. It lives on your computer, not inside the container. Starting again with `docker compose up -d` will pick up right where you left off.

---

## Troubleshooting

### "Empty library" after scan

Your library path isn't reaching the container. Check:

1. The `LIBRARY_PATH` in `.env` is correct and matches the actual folder on your computer.
2. On macOS/Windows: the path is in Docker Desktop's File Sharing allow-list (Docker Desktop > Settings > Resources > File Sharing).
3. The folder contains CBZ/CBR/CB7 files (LongBox doesn't scan other formats like PDF or EPUB).
4. Run this command to verify the container can see your files:
   ```bash
   docker exec longbox ls /library
   ```
   If this prints nothing, the volume mount isn't working.

### SABnzbd connection refused

The SABnzbd base URL must use `host.docker.internal` instead of `localhost`:

```
http://host.docker.internal:8080
```

This is because `localhost` inside the container refers to the container itself, not your computer. `host.docker.internal` is Docker's way of saying "the computer running this container."

**Linux note:** `host.docker.internal` may not work on older Docker versions. Use your machine's LAN IP instead (e.g., `http://192.168.1.100:8080`), or uncomment the `extra_hosts` line in your docker-compose.yml and restart.

### Windows path issues

If you see mount errors on Windows:

- Make sure paths in `.env` use forward slashes: `C:/Users/sam/Comics` (not backslashes)
- Make sure the drive is shared in Docker Desktop > Settings > Resources > File Sharing
- If using WSL 2, your files are also accessible at `/mnt/c/Users/sam/Comics` from inside WSL

### ComicVine rate limiting

The CV rate chip in the top-right of the LongBox UI shows your remaining budget (e.g., "CV 42/180"). If you hit the limit, LongBox will pause and resume automatically. Large initial enrichment runs (hundreds of series) may take a few hours to complete. This is normal.

---

## Library organization

LongBox works best when your library is organized by series:

```
Comics/
  Saga (2012)/
    Saga (2012) 001.cbz
    Saga (2012) 002.cbr
    Saga (2012) 003.cbz
  Absolute Batman (2024)/
    Absolute Batman (2024) 001.cbr
    Absolute Batman (2024) 002.cbz
```

The ideal filename format is `Series Name (Year) Issue.ext`. LongBox's scanner handles many naming variations, but this format gives the best matching results.

LongBox does NOT require this structure. It will scan and attempt to match any CBZ/CBR/CB7 file it finds, regardless of folder structure or naming convention. But organized libraries match faster and more accurately.

---

## Settings reference

All settings are configurable from the LongBox UI at Settings > Tunable Settings. Changes take effect on the next scan or sweep without restarting.

| Setting | Default | Description |
|---|---|---|
| Match confidence threshold | 0.75 | Minimum similarity score for a file to be marked "owned" vs "needs review" |
| Pull indexer match threshold | 0.75 | Minimum similarity for NZB title matching during automated search |
| Minimum file size (MB) | 35 | Files smaller than this are rejected as likely corrupt/partial |
| Pull exclusion keywords | Infinity Comic, Trade Paperback, Hardcover, Omnibus, Compendium | NZB titles containing these terms are skipped |

---

## Getting help

LongBox is open source. File issues and feature requests on the [GitHub repository](https://github.com/longbox-app/longbox).
