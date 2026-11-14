# Debrid service accounts

A debrid service, such as Real-Debrid, AllDebrid, Premiumize, TorBox or Mega-Debrid, is a paid account that downloads files from file hosts
on its own servers. It hands back a plain direct link, without the host's waits, speed caps
and captchas. With your account in MDM, a link from a host your service supports is unlocked
through the service, then downloaded by MDM like any file: all its connections, pause and
resume.

MDM supports five services: **Real-Debrid**, **AllDebrid**, **Premiumize**, **TorBox** and **Mega-Debrid**.

> **Experimental:** built from each service's public API documentation, not yet confirmed with a
> real paid account. AllDebrid's answers were also checked against its real API with the public
> demo key from its documentation, which answers with sample data, not a real file. If something
> doesn't work with your account, please
> [open an issue](https://github.com/anefzaoui/misterdownloadmanager/issues) (never paste your
> token or key in it).

## Add your account

1. **Options → Accounts → Add...**
2. Pick the service, then either:
   - **Sign in in the browser** (Real-Debrid, AllDebrid and TorBox): MDM shows a code and opens
     the service's page (TorBox's is also at `tor.box/link`). Type the code there (AllDebrid's
     page already has it) and allow MDM. The dialog notices on its own.
   - **User name** and **Password**, then **Sign in** (Mega-Debrid). Mega-Debrid blocks your
     connection for some minutes after a few wrong passwords. Each sign-in replaces the
     account's last one at Mega-Debrid, so another program signed in to the same account makes
     MDM sign in again on its next link, on its own. MDM itself signs in only when its token is
     refused, so its checks and its downloads at once don't end each other's. MDM keeps your
     password (in the keychain, see below) to sign in again.
   - or paste your **API token** (**Find my token** opens the service's page that shows it),
     then **Use the token**. Premiumize signs in this way only: paste the API key from its
     Account page. On Mega-Debrid, make an application under **My Account → API Key** and paste
     its key; MDM then never needs your password, and doesn't keep it: the key is all it stores,
     and you can delete the key on that page at any time. Mega-Debrid doesn't say whether a
     key's account is premium, so the Accounts list says **the service doesn't say whether it
     is premium** for it; a free account's links are refused when you download them.
3. MDM checks the account and shows its name and whether it is premium. **OK** keeps it.

The list's order is the order accounts are asked: a link goes through the first account
whose service supports its host. **Up** and **Down** change it.

**Download supported file-host links through these accounts** turns it all off and on.

## Torrents through the service

Tick **Send torrents and magnet links to the first account instead of to peers** in
**Options → Accounts**. A magnet link (Add URL, the browser, the clipboard, a drop) or a
`.torrent` file (Add torrent's **Download Now** or **Later**) then goes to the first account in
the list whose service takes torrents (Mega-Debrid doesn't, yet):

1. The service downloads the torrent on its own servers. The list shows how far it is, such as
   **45% at Real-Debrid**. A torrent the service already has is ready at once.
2. MDM then downloads its files from the service over HTTP, as any download, into the
   **Torrents** folder. A torrent of several files gets a folder of its own there.
3. Once every file is here, MDM removes the torrent from the account's list. On Premiumize,
   only the transfer's record goes: its files stay in your cloud.

- Your computer never connects to the torrent's peers.
- A `.torrent` file is sent as its magnet link (its hash, its name and its trackers). Every file
  of the torrent is downloaded; the service fetches the whole torrent.
- Stopped, or after a restart, the download continues where it was: the service isn't sent the
  torrent again, and files already downloaded aren't downloaded again.
- A file name that would land outside the torrent's folder (`../`) stops the download, and
  nothing is written.
- If the service can't get the torrent (no peers, too big, refused), the list says why.

## What happens to a link

- Add URL, the clipboard, the browser's **Download with Mister Download Manager** link menu,
  and batch downloads go through the account when its service supports
  the link's host.
- A download the browser had already started (MDM took it over from the browser) is
  downloaded as it is: the host is already sending the file.
- The command line's `get` and `resume` use the same accounts, and say `through Real-Debrid`;
  `--direct` downloads the address itself ([command line](command-line.md)).
- The list keeps the host's link. The direct link is never shown or copied: the services
  forbid sharing it. It lives only in the download's resume data, and when it expires MDM asks
  the service for a new one.
- A link the service can't give says why in the list: sign in again, premium needed, the host
  is down (MDM tries again later), the file is gone, and so on.
- AllDebrid sometimes needs a while to prepare a link. MDM waits for it, up to 10 minutes,
  and the list says so.
- TorBox first downloads the file to its own servers (at once when it has it already). MDM
  waits for it, up to an hour, and the list says how far it is, such as **40% at TorBox**.
  Once the download is done, or taken off MDM's list, it leaves your account's list of web
  downloads on TorBox; a paused one keeps its place there, to go on with.
- MDM asks each service well under its request limit: Real-Debrid at most 120 times a minute
  (it allows 250), AllDebrid 300 (it allows 600), Premiumize 60 (it publishes no limit), TorBox
  120 (it allows 300 a minute, and 60 new web downloads an hour), Mega-Debrid 60 (it allows 50
  a second). When a service says it's asked too often, MDM waits.

## Download one link directly instead

- **Download File Info** says which account a link goes through: **Download through
  Real-Debrid (your account)**, ticked. Untick it to download that one link directly, without
  the service. MDM looks the address up again the new way, so the name and size shown are the
  ones you will get. A download the browser had already started shows it unticked.
- For a download already in the list, **File Properties** has the same choice beside the login:
  **Through Real-Debrid**. It can be changed while the download isn't running. The next start
  goes the new way, and continues from what is already downloaded when the file is the same.
- MDM keeps the choice with the download, after a restart too.

## Your account, your rules

- Use only your own account. Sharing an account or its links breaks the services' terms and
  can get the account suspended.
- MDM sends the service only the links you download through it, and your account's token or
  key (on Mega-Debrid, or its user name and password). Nothing else. Mega-Debrid's
  documentation puts them in the address; MDM sends them in the request's body instead, where
  no proxy or server log keeps them.
- MDM ships no list of hosts or links; the service says which hosts it supports.

## Where the sign-in is kept

In the system's keychain, as are Sites Logins' passwords and the proxy's. MDM's settings keep
only a reference to it. **Options → Accounts** says where they are on your computer.

- **Windows**: in **Windows Credential Manager** (Control Panel → Credential Manager → Windows
  Credentials), as entries named `Mister Download Manager/…`, for your Windows account. They
  roam with your profile, as MDM's settings folder does.
- **Linux**: in the system keyring through the Secret Service (GNOME Keyring, KeePassXC, KDE's
  wallet with its Secret Service on), its default collection, as items labelled
  `Mister Download Manager: …`. MDM never unlocks a locked keyring and never asks for its
  password: when the keyring is locked (some desktops that log in on their own leave it
  locked) or there is none (many tiling window managers), the sign-ins stay in MDM's settings,
  in a data folder only you may read (0700), as before.
- **macOS**: in the login keychain (**Keychain Access**), service `Mister Download Manager`.
  macOS may ask once whether MDM may use them, after an update.
- **A portable copy** writes nothing outside its folder: it keeps them in its own settings
  (Windows: encrypted for your Windows account).

Sign-ins saved by an earlier version move to the keychain on the next start; the settings keep
the old copy until the keychain has it. When the keychain can't be read (a locked keyring), the
account shows **in the locked keyring** and comes back once the keyring is unlocked and MDM
starts again. A removed account's entry leaves the keychain with it.

**Uninstalling** on Windows with **remove the download list and settings** removes MDM's
entries from Credential Manager too. Elsewhere, or to remove them yourself:
`misterdownloadmanager --forget-sign-ins` (only MDM's own entries).

MDM never writes the token to its event log, crash reports, the automation API or the
command line's output.
