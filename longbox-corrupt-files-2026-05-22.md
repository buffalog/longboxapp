# Corrupt comic files — 2026-05-22

81 files in `/Volumes/Comics` failed archive extraction during the CBR hot-fix backfill scan (scan run 22, 4930 files indexed). LongBox still catalogued every one of them via filename match — they are in the catalog and visible — but no `ComicInfo.xml` could be read, so matching for these issues is filename-only. Each is a re-acquisition candidate.

## Failure breakdown

- **Not a RAR archive** — 39
- **File header damaged** — 36
- **Could not find EOCD** — 4
- **Invalid CDFH offset in EOCD** — 2

*File header damaged* — a genuine RAR archive, truncated or damaged. *Not a RAR archive* — the `.cbr` is not valid RAR (typically a zero-byte or garbage file). *Could not find EOCD* / *Invalid CDFH offset in EOCD* — the `.cbz` is a corrupt or zero-byte ZIP.

## Files

| File | Reason |
|------|--------|
| A Righteous Thirst For Vengeance/A Righteous Thirst For Vengeance 001.cbr | File header damaged |
| A Righteous Thirst For Vengeance/A Righteous Thirst For Vengeance 002.cbr | File header damaged |
| Adventureman (2020)/Adventureman (2020) 006.cbr | Not a RAR archive |
| American Vampire (2010)/American Vampire (2010) 003.cbz | Invalid CDFH offset in EOCD |
| Ascender/Ascender 015.cbr | File header damaged |
| Ascender/Ascender 016.cbr | Not a RAR archive |
| Ascender/Ascender 017.cbr | File header damaged |
| Basilisk/Basilisk 002.cbr | File header damaged |
| Basilisk/Basilisk 003.cbr | Not a RAR archive |
| Basilisk/Basilisk 004.cbr | Not a RAR archive |
| Bedlam/Bedlam 007.cbr | File header damaged |
| Chew/Chew 023.cbr | Not a RAR archive |
| Chew/Chew 033.cbr | Not a RAR archive |
| Chew/Chew 034.cbr | File header damaged |
| Clone/Clone 008.cbr | File header damaged |
| Daredevil (2023)/Daredevil (2023) 031.cbr | Not a RAR archive |
| Daredevil (2023)/Daredevil (2023) 032.cbr | Not a RAR archive |
| Daredevil (2023)/Daredevil (2023) 033.cbr | Not a RAR archive |
| Daredevil (2023)/Daredevil (2023) 035.cbr | Not a RAR archive |
| Deadly Class/Deadly Class 018.cbr | File header damaged |
| Deadly Class/Deadly Class 034.cbr | File header damaged |
| Deadly Class/Deadly Class 038.cbr | File header damaged |
| Deadly Class/Deadly Class 047.cbr | Not a RAR archive |
| Deathstroke Inc. (2021)/Deathstroke Inc 002.cbr | Not a RAR archive |
| Ex Machina/Ex Machina 006.cbr | File header damaged |
| G.O.D.S (2023)/G.O.D.S (2023) 002.cbz | Could not find EOCD |
| Great Pacific/Great Pacific 007.cbz | Invalid CDFH offset in EOCD |
| Ice Cream Man (2018)/Ice Cream Man (2018) 001.cbr | File header damaged |
| Ice Cream Man (2018)/Ice Cream Man (2018) 026.cbr | File header damaged |
| Ice Cream Man (2018)/Ice Cream Man (2018) 043.cbz | Could not find EOCD |
| Incredible Hulk/Incredible Hulk 012.cbr | Not a RAR archive |
| Inferno (2021) (2021)/Inferno 002.cbr | Not a RAR archive |
| Jupiter's Legacy Requiem/Jupiter's Legacy Requiem 001.cbr | File header damaged |
| Jupiter's Legacy Requiem/Jupiter's Legacy Requiem 002.cbr | Not a RAR archive |
| Killadelphia/Killadelphia 014.cbr | File header damaged |
| Killadelphia/Killadelphia 015.cbr | Not a RAR archive |
| Killadelphia/Killadelphia 016.cbr | Not a RAR archive |
| Killadelphia/Killadelphia 018.cbr | Not a RAR archive |
| Knights Temporal/Knights Temporal 004.cbr | File header damaged |
| Mind the Gap/Mind the Gap 010.cbr | File header damaged |
| Mind the Gap/Mind the Gap 017.cbr | File header damaged |
| Monstress/Monstress 035.cbr | Not a RAR archive |
| Morning Glories/Morning Glories 026.cbr | File header damaged |
| Morning Glories/Morning Glories 027.cbr | File header damaged |
| Morning Glories/Morning.Glories.013.2011.digital-Empire.cbr | File header damaged |
| Morning Glories/Morning.Glories.016.2012.digital-TheGroup.cbr | File header damaged |
| Morning Glories/Morning.Glories.034.2013.Digital.Darkness-Empire.cbr | File header damaged |
| Nocterra/Nocterra 004.cbr | File header damaged |
| Nocterra/Nocterra 005.cbr | File header damaged |
| Nowhere Men/Nowhere Men 001.cbr | File header damaged |
| Nowhere Men/Nowhere Men 004.cbr | File header damaged |
| Oblivion Song/Oblivion Song 032.cbr | Not a RAR archive |
| Oblivion Song/Oblivion Song 033.cbr | Not a RAR archive |
| Oblivion Song/Oblivion Song 034.cbr | Not a RAR archive |
| Once & Future/Once & Future 025.cbr | Not a RAR archive |
| Once & Future/Once & Future 026.cbr | Not a RAR archive |
| Ordinary Gods/Ordinary Gods 001.cbr | File header damaged |
| Ordinary Gods/Ordinary Gods 002.cbr | Not a RAR archive |
| Ordinary Gods/Ordinary Gods 005.cbr | Not a RAR archive |
| Sex/Sex 003.cbr | File header damaged |
| Sex/Sex 014.cbr | File header damaged |
| Something is Killing the Children (2019)/Something is Killing the Children (2019) 004.cbr | File header damaged |
| Something is Killing the Children (2019)/Something is Killing the Children (2019) 017.cbr | File header damaged |
| Syphon/Syphon 001.cbr | Not a RAR archive |
| Syphon/Syphon 002.cbr | Not a RAR archive |
| The Department of Truth (2022)/The Department of Truth (2022) 011.cbr | Not a RAR archive |
| The Department of Truth (2022)/The Department of Truth (2022) 012.cbr | Not a RAR archive |
| The Nice House on the Lake/The Nice House on the Lake 002.cbr | Not a RAR archive |
| The Nice House on the Lake/The Nice House on the Lake 003.cbr | File header damaged |
| The Nice House on the Lake/The Nice House on the Lake 006.cbz | Could not find EOCD |
| The Silver Coin/The Silver Coin 004.cbr | Not a RAR archive |
| The Silver Coin/The Silver Coin 005.cbr | Not a RAR archive |
| The Twilight Zone (2025)/The Twilight Zone (2025) 001.cbr | Not a RAR archive |
| The Twilight Zone (2025)/The Twilight Zone (2025) 002.cbz | Could not find EOCD |
| The Twilight Zone (2025)/The Twilight Zone (2025) 006.cbr | Not a RAR archive |
| The Walking Dead Deluxe (2020)/The Walking Dead Deluxe (2020) 026.cbr | File header damaged |
| The Woods (2014)/The Woods (2014) 021.cbr | Not a RAR archive |
| We Only Find Them When They're Dead/We Only Find Them When They're Dead 007.cbr | Not a RAR archive |
| We Only Find Them When They're Dead/We Only Find Them When They're Dead 008.cbr | Not a RAR archive |
| What's The Furthest Place From Here/What's The Furthest Place From Here 001.cbr | Not a RAR archive |
| Wytches/Wytches 005.cbr | File header damaged |
