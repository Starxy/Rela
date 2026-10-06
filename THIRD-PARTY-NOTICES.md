# Third-party runtime notices

Rela currently packages unmodified files from the official EasyTier 2.7.0-0a783c8e Windows x64 development build. The archive SHA-256 and individual binary hashes are pinned in config/easytier-version.json. This is an internal development package, not a cleared public release.

## EasyTier Core and CLI

Source and release: https://github.com/EasyTier/EasyTier/tree/0a783c8e04561d1fee4e3e922e9576402d5bfea3

Copyright belongs to the EasyTier contributors. The upstream project supplies the GNU Lesser General Public License version 3. The LGPL and GPL texts are included in third-party-licenses. Source corresponding to the pinned build is available at https://github.com/EasyTier/EasyTier/archive/0a783c8e04561d1fee4e3e922e9576402d5bfea3.tar.gz . No EasyTier source modifications are made by this project. Distribution must preserve applicable notices and corresponding-source obligations.

## Wintun 0.14.1

Copyright © 2018–2021 WireGuard LLC. Wintun is a trademark of Jason A. Donenfeld.

Project: https://www.wintun.net/

The signed DLL is supplied by the official EasyTier release. The Wintun binary license from the official 0.14.1 archive is included as third-party-licenses/Wintun-LICENSE.txt. The binary license differs from the source code license.

## WinDivert 2.2.2

Copyright © Basil 2011–2022.

Project and source: https://github.com/basil00/WinDivert/tree/v2.2.2

License text is included as third-party-licenses/WinDivert-LICENSE.txt. The exact driver is the unmodified WinDivert64.sys from the pinned EasyTier archive; its SHA-256 matches the x64 driver in the official WinDivert-2.2.2-A.zip. Corresponding source is available at https://github.com/basil00/WinDivert/archive/refs/tags/v2.2.2.tar.gz . EasyTier and WinDivert source archives accompany the Rela 0.1.0 Release assets.

## Packet.dll / Npcap 1.79 — distribution scope

The DLL's version metadata identifies Npcap 1.79 and Copyright (c) 2023, Insecure.Com LLC.

Npcap is not licensed under EasyTier's LGPL. The [Npcap license](https://github.com/nmap/npcap/blob/master/LICENSE) places separate restrictions on use and redistribution. Bundling this DLL in an upstream archive does not establish Rela's redistribution permission.

The release plan in todo.md (T45) records existing permission for internal distribution. On 2026-09-29 the source repository was made public. The scope recorded for internal distribution has not been documented here as covering publicly downloadable release assets; align the package's distribution scope and these notices before publishing those assets. Current packaging must not be presented as a cleared public release. No Npcap driver installer is run by Rela.

## Application dependencies

JavaScript and Rust dependencies are pinned by package-lock.json and Cargo.lock. A complete release notice inventory and license review for those transitive dependencies remains part of release preparation.
