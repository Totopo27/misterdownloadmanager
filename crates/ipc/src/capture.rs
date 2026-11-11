//! What the browser extension may hand to Mister Download Manager: Options → General ("Use
//! advanced browser integration", the browsers to capture from) and Options → File types (the
//! types to take, the sites and addresses to leave alone). One implementation for the app and
//! the native messaging host: the host reads the app's saved settings at every hand-off (so a
//! change applies at once) and answers "not taken" for the rest, and the extension then lets the
//! browser download it (extension/background.js).

use serde::Deserialize;

/// Options → File types defaults: IDM 6.42's; TORRENT (a .torrent the browser hands over opens
/// Add torrent, #39); and today's installers, packages and archives (macOS DMG and PKG, Linux
/// DEB, RPM and AppImage, Windows MSIX, APPX and CAB, Java's JAR, XZ, Zstandard and their tarballs)
/// and open audio and video formats (WEBM, FLAC, OPUS; #208); and what people fetch by the
/// gigabyte today (#382): AI models and datasets, 3D and CAD, camera RAW and design files, books
/// and Office files, disk images and split archives. Never what a browser saves as part of
/// everyday browsing (pages, scripts, plain text, web images) or installs itself (XPI, CRX).
pub const DEFAULT_AUTO_TYPES: &str = "00* 3DM 3DS 3GP 3MF 7Z AAB AAC ABC ACE AFDESIGN AFPHOTO AI AIF AIFF APE APK APKM APKS APPIMAGE APPX APPXBUNDLE ARJ ARROW ARW ASF AU AVI AVRO AZW AZW3 BAK BAM BIN BLEND BZ BZ2 C4D CAB CAF CBR CBZ CCD CHD CKPT CPIO CR2 CR3 CRAM CSO DAE DAT DCM DDS DEB DFF DIVX DJVU DMG DNG DOC DOCX DSF DWG DXF EPS EPUB EXE EXR F3D F4V FASTA FASTQ FB2 FBX FEATHER FIG FITS FLAC FLATPAK FQ GCODE GCZ GEM GGML GGUF GLB GLTF GPKG GRIB GRIB2 GZ GZIP H5 HDF5 HDR IGES IGS IMG INDD IPA ISO ISZ JAR JOBLIB JSONL KERAS KRA LAS LAZ LLAMAFILE LWO LZ LZ4 LZH LZMA M2T M2TS M2V M4A M4B M4V MA MAT MAX MB MBTILES MDF MDS MID MIDI MKA MKV MLMODEL MOBI MOV MP3 MP4 MPA MPE MPEG MPG MSI MSIX MSIXBUNDLE MSP MSU MTS MXF NC NDJSON NEF NEMO NII NPY NPZ NRG NSP NSZ NUPKG OBJ ODP ODS ODT OGG OGM OGV ONNX OPUS ORC ORF OTF OVA OVF PAK PARQUET PB PBF PDF PICKLE PK3 PK4 PKG PKL PLJ PLY POTX PPS PPSX PPT PPTX PSB PSD PT PTH QCOW QCOW2 QT R0* R1* R2* R3* R4* R5* R6* R7* R8* R9* RA RAF RAR RDS RM RMVB RPM RTF RUN RVZ RW2 SAFETENSORS SEA SF2 SFZ SHP SIT SITX SKETCH SKP SLDASM SLDPRT SNAP SQLITE SQLITE3 STEP STL STP TAR TBZ TBZ2 TFLITE TGA TGZ TIF TIFF TLZ TORRENT TTA TTC TTF TXZ TZST UNITYPACKAGE USD USDA USDC USDZ VDB VDI VHD VHDX VMDK VOB VPK VSIX WAV WBFS WEBM WHL WIM WMA WMV WRL WTV WUA WV X3D XAPK XCF XCI XLS XLSB XLSM XLSX XZ Z Z0* ZIP ZIPX ZPR ZST ZTL";

/// The lists earlier versions started with: IDM 6.42's (MDM 0.2.x), with TORRENT (#39), then
/// with today's packages and open formats (#208). A saved list equal to one of them was never edited, and becomes today's (`upgraded_types`).
const EARLIER_AUTO_TYPES: [&str; 3] = [
    "3GP 7Z AAC ACE AIF APK ARJ ASF AVI BIN BZ2 EXE GZ GZIP IMG ISO LZH M4A M4V MKV MOV MP3 MP4 MPA MPE MPEG MPG MSI MSU OGG OGV PDF PLJ PPS PPT QT R0* R1* RA RAR RM RMVB SEA SIT SITX TAR TIF TIFF WAV WMA WMV Z ZIP",
    "3GP 7Z AAC ACE AIF APK ARJ ASF AVI BIN BZ2 EXE GZ GZIP IMG ISO LZH M4A M4V MKV MOV MP3 MP4 MPA MPE MPEG MPG MSI MSU OGG OGV PDF PLJ PPS PPT QT R0* R1* RA RAR RM RMVB SEA SIT SITX TAR TIF TIFF TORRENT WAV WMA WMV Z ZIP",
    "3GP 7Z AAC ACE AIF APK APPIMAGE APPX APPXBUNDLE ARJ ASF AVI BIN BZ2 CAB DEB DMG EXE FLAC GZ GZIP IMG ISO JAR LZH M4A M4V MKV MOV MP3 MP4 MPA MPE MPEG MPG MSI MSIX MSU OGG OGV OPUS PDF PKG PLJ PPS PPT QT R0* R1* RA RAR RM RMVB RPM SEA SIT SITX TAR TGZ TIF TIFF TORRENT TXZ WAV WEBM WMA WMV XZ Z ZIP ZST",
];

/// Today's default list when `saved` (Options → File types as saved) is an earlier version's
/// default, untouched (the same types, in any order or case); `None` when it was edited, or is
/// today's already: the user's own list never changes.
pub fn upgraded_types(saved: &str) -> Option<&'static str> {
    let set = |s: &str| {
        let mut v: Vec<String> = s.split_whitespace().map(str::to_ascii_uppercase).collect();
        v.sort();
        v.dedup();
        v
    };
    let saved = set(saved);
    EARLIER_AUTO_TYPES
        .iter()
        .any(|old| set(old) == saved)
        .then_some(DEFAULT_AUTO_TYPES)
}
pub const DEFAULT_EXCLUDED_SITES: &str = "*.update.microsoft.com download.windowsupdate.com *.download.windowsupdate.com siteseal.thawte.com ecom.cimetz.com *.voice2page.com";

/// The capture settings, named as the app saves them (`Settings` in downloads.json).
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct CaptureRules {
    pub advanced_integration: bool,
    /// (browser name as Options lists it, capture from it?)
    pub browsers: Vec<(String, bool)>,
    pub auto_types: String,
    pub excluded_sites: String,
    pub excluded_addresses: Vec<String>,
    /// Options → General → Download panels: the download button over videos in the browsers.
    pub video_panel: bool,
    /// The sites it stays off (space-separated, `*.example.com` takes subdomains too).
    pub video_panel_sites: String,
    /// Options → General: also take the listed types the browser would open itself (a PDF in
    /// its viewer, an MP3 in its player), not only what it downloads.
    pub capture_opened: bool,
    /// Options → File types: files smaller than this (KB) stay with the browser (a receipt, an
    /// icon); 0 takes any size. Only when the browser knows the size.
    pub capture_min_kb: u64,
}

impl Default for CaptureRules {
    fn default() -> Self {
        CaptureRules {
            advanced_integration: true,
            browsers: Vec::new(),
            auto_types: DEFAULT_AUTO_TYPES.into(),
            excluded_sites: DEFAULT_EXCLUDED_SITES.into(),
            excluded_addresses: Vec::new(),
            video_panel: true,
            video_panel_sites: String::new(),
            capture_opened: true,
            capture_min_kb: 0,
        }
    }
}

/// Why a download is left to the browser.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Declined {
    IntegrationOff,
    Browser,
    FileType,
    Site,
    Address,
    Small,
}

impl Declined {
    pub const ALL: [Declined; 6] = [
        Declined::IntegrationOff,
        Declined::Browser,
        Declined::FileType,
        Declined::Site,
        Declined::Address,
        Declined::Small,
    ];

    /// Its name between the native host and the extension: the host's reply (`declined`), and
    /// the extension's notice of a download it left alone by the same rules
    /// (extension/background.js).
    pub fn code(self) -> &'static str {
        match self {
            Declined::IntegrationOff => "integration-off",
            Declined::Browser => "browser",
            Declined::FileType => "file-type",
            Declined::Site => "site",
            Declined::Address => "address",
            Declined::Small => "small",
        }
    }

    pub fn from_code(code: &str) -> Option<Declined> {
        Declined::ALL.into_iter().find(|d| d.code() == code)
    }

    /// Why, for MDM's log (English, as the rest of it), naming the setting that decides.
    pub fn reason(self) -> &'static str {
        match self {
            Declined::IntegrationOff => {
                "Options > General > Use advanced browser integration is off"
            }
            Declined::Browser => {
                "the browser is unticked in Options > General > Capture downloads from the following browsers"
            }
            Declined::FileType => "its type isn't in Options > File types",
            Declined::Site => {
                "its site is in Options > File types > Don't start downloading automatically from the following sites"
            }
            Declined::Address => {
                "its address is in Options > File types > Edit list (addresses left to the browser)"
            }
            Declined::Small => {
                "it is smaller than Options > File types > Don't take files smaller than"
            }
        }
    }
}

impl CaptureRules {
    /// The rules as the app last saved them (defaults when there is no list yet).
    pub fn load() -> CaptureRules {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Stored {
            settings: CaptureRules,
        }
        crate::data_dir()
            .and_then(|d| std::fs::read(d.join("downloads.json")).ok())
            .and_then(|b| serde_json::from_slice::<Stored>(&b).ok())
            .map(|s| s.settings.upgraded())
            .unwrap_or_default()
    }

    /// An earlier version's default File types list, untouched, as today's (the app does the
    /// same when it reads its list, and saves it so).
    pub fn upgraded(mut self) -> CaptureRules {
        if let Some(types) = upgraded_types(&self.auto_types) {
            self.auto_types = types.into();
        }
        self
    }

    /// Is a file of `size` bytes (the browser's figure, when it has one) too small to take?
    pub fn too_small(&self, size: Option<u64>) -> bool {
        size.is_some_and(|s| s > 0 && s < self.capture_min_kb.saturating_mul(1024))
    }

    /// Does the file name's extension appear in File types (`R0*` patterns too)?
    pub fn auto_type(&self, name: &str) -> bool {
        type_listed(&self.auto_types, name)
    }

    /// Are downloads from `browser` (a name `browser_named` gives) taken at all?
    pub fn captures(&self, browser: &str) -> bool {
        self.capture_off(browser).is_none()
    }

    /// Why no download from `browser` is taken (integration off, the browser unticked), if so.
    pub fn capture_off(&self, browser: &str) -> Option<Declined> {
        if !self.advanced_integration {
            Some(Declined::IntegrationOff)
        } else if self.browsers.iter().any(|(b, on)| b == browser && !on) {
            Some(Declined::Browser)
        } else {
            None
        }
    }

    /// May `browser` hand `url` (saved as `file_name`) over?
    pub fn accepts(&self, url: &str, file_name: &str, browser: &str) -> Result<(), Declined> {
        if let Some(off) = self.capture_off(browser) {
            return Err(off);
        }
        if !self.auto_type(file_name) {
            return Err(Declined::FileType);
        }
        let host = host_of(url);
        if self
            .excluded_sites
            .split_whitespace()
            .any(|p| host_matches(&host, p))
        {
            return Err(Declined::Site);
        }
        if self
            .excluded_addresses
            .iter()
            .any(|p| address_matches(p, url))
        {
            return Err(Declined::Address);
        }
        Ok(())
    }
}

impl CaptureRules {
    /// May the download button show over videos on `page` (its address) in `browser`?
    /// Integration on, that browser captured, the button on and the site not one it stays off.
    /// `top`: the tab's address when `page` is a frame of another page's (a player embedded from
    /// another site): the site in the address bar is the one a person lists (#532).
    pub fn panel_on(&self, page: &str, top: Option<&str>, browser: &str) -> bool {
        self.captures(browser)
            && self.video_panel
            && !self.panel_hidden_on(page)
            && !top.is_some_and(|t| !t.is_empty() && self.panel_hidden_on(t))
    }

    /// Is `page` (an address) on a site the button stays off? A name without `*.` takes its
    /// subdomains too (#532: `youtube.com` listed, the button still showed on www.youtube.com).
    /// Supports wildcards (`*`) like `*.youtube.*` or `youtube.*` (#759).
    pub fn panel_hidden_on(&self, page: &str) -> bool {
        let host = host_of(page);
        self.video_panel_sites.split_whitespace().any(|p| {
            panel_site_matches(&host, p)
                || (!p.starts_with('*') && panel_site_matches(&host, &format!("*.{p}")))
        })
    }
}

/// `sites` (space-separated) with `host` added, unless a pattern already covers it or it isn't
/// one host name (a space would add several patterns, `*` all sites).
pub fn add_site(sites: &str, host: &str) -> String {
    let host = host.trim().to_ascii_lowercase();
    let one_host = !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '[' | ']'));
    if !one_host || sites.split_whitespace().any(|p| host_matches(&host, p)) {
        return sites.to_owned();
    }
    let mut out = sites.trim().to_owned();
    if !out.is_empty() {
        out.push(' ');
    }
    out.push_str(&host);
    out
}

/// `sites` without the names that stand for `host` alone (the video button's "Turn on", undoing
/// its "Never on <site>"): a pattern for more sites (`*.example.com`) stays, as the user wrote it
/// for all of them.
pub fn remove_site(sites: &str, host: &str) -> String {
    let host = host.trim().to_ascii_lowercase();
    sites
        .split_whitespace()
        .filter(|p| p.contains('*') || !host_matches(&host, p))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The sites a list holds (space-separated), as a file: one a line (Options → Download panels,
/// #424).
pub fn sites_to_text(sites: &str) -> String {
    sites.split_whitespace().fold(String::new(), |mut out, s| {
        out.push_str(s);
        out.push('\n');
        out
    })
}

/// `sites` with the patterns of a file (one or more names a line, or space-separated) added, those
/// not there already whatever their case, and how many that was. A name holding anything else
/// than a host pattern's characters is left out.
pub fn merge_sites(sites: &str, text: &str) -> (String, usize) {
    let mut out: Vec<String> = sites.split_whitespace().map(str::to_owned).collect();
    let mut added = 0;
    for name in text.split_whitespace().take(5000) {
        let usable = name.len() <= 253
            && name.chars().all(|c| {
                c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '[' | ']' | '*' | '/')
            });
        if usable && !out.iter().any(|s| s.eq_ignore_ascii_case(name)) {
            out.push(name.to_owned());
            added += 1;
        }
    }
    (out.join(" "), added)
}

/// Does `name`'s extension appear in `types` (Options → File types: `EXE ZIP R0*`)?
pub fn type_listed(types: &str, name: &str) -> bool {
    let Some((_, ext)) = name.rsplit_once('.') else {
        return false;
    };
    let ext = ext.to_ascii_uppercase();
    types.split_whitespace().any(|t| {
        let t = t.to_ascii_uppercase();
        match t.strip_suffix('*') {
            Some(prefix) => ext.starts_with(prefix),
            None => ext == t,
        }
    })
}

/// The browser by the names Options lists, from its User-Agent and, for those that say who
/// they are only there, its `brands`: `navigator.userAgentData`'s (Brave, Opera), and for
/// Firefox's forks the name `runtime.getBrowserInfo` gives (Zen, LibreWolf, Floorp, Waterfox;
/// extension/background.js). Helium names itself Google Chrome on purpose.
pub fn browser_named(user_agent: &str, brands: &[String]) -> &'static str {
    let brand = |b: &str| brands.iter().any(|x| x == b);
    if brand("Zen") {
        "Zen Browser"
    } else if brand("LibreWolf") {
        "LibreWolf"
    } else if brand("Floorp") {
        "Floorp"
    } else if brand("Waterfox") {
        "Waterfox"
    } else if brand("Brave") {
        "Brave Browser"
    } else if brand("Opera") || brand("Opera GX") {
        "Opera"
    } else if brand("Microsoft Edge") {
        "Microsoft Edge"
    } else {
        browser_of(user_agent)
    }
}

/// The browser a User-Agent belongs to, by the names Options lists. Brave says it is Chrome.
pub fn browser_of(user_agent: &str) -> &'static str {
    if user_agent.contains("Edg/") {
        "Microsoft Edge"
    } else if user_agent.contains("OPR/") || user_agent.contains("Opera") {
        "Opera"
    } else if user_agent.contains("Firefox/") {
        "Mozilla Firefox"
    } else if user_agent.contains("Chrome/") {
        "Google Chrome"
    } else if user_agent.contains("Safari/") {
        "Apple Safari"
    } else {
        ""
    }
}

/// The host of an http(s) address, lower case.
pub fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if host.starts_with('[') {
        host.split(']').next().map_or(host, |h| &h[1..])
    } else {
        host.split(':').next().unwrap_or(host)
    };
    host.to_ascii_lowercase()
}

/// `host` is `pattern` (case-insensitive) or, for `*.example.com`, one of its subdomains.
pub fn host_matches(host: &str, pattern: &str) -> bool {
    let pattern = pattern.trim().to_ascii_lowercase();
    let pattern = pattern
        .split_once("://")
        .map_or(pattern.as_str(), |(_, r)| r);
    let pattern = pattern.split('/').next().unwrap_or("");
    match pattern.strip_prefix("*.") {
        Some(base) => host == base || host.ends_with(&format!(".{base}")),
        None => !pattern.is_empty() && host == pattern,
    }
}

/// `host` matches download panel site `pattern` (case-insensitive), supporting wildcards (`*`)
/// like `*.youtube.*`, `youtube.*` or `*dailymotion*` (#759) and, for `*.example.com`,
/// matching both the domain and its subdomains.
pub fn panel_site_matches(host: &str, pattern: &str) -> bool {
    let pattern = pattern.trim().to_ascii_lowercase();
    let pattern = pattern
        .split_once("://")
        .map_or(pattern.as_str(), |(_, r)| r);
    let pattern = pattern.split('/').next().unwrap_or("");
    if pattern.is_empty() {
        return false;
    }
    match pattern.strip_prefix("*.") {
        Some(base) => wildcard(base, host) || wildcard(pattern, host),
        None => wildcard(pattern, host),
    }
}

/// An address pattern (Options → File types) against `url`, case-insensitive: written with a
/// scheme (`https://example.com/private/*`) it is the whole address; without one
/// (`example.com/private/*`), the address after its `scheme://`.
pub fn address_matches(pattern: &str, url: &str) -> bool {
    let (pattern, url) = (
        pattern.trim().to_ascii_lowercase(),
        url.to_ascii_lowercase(),
    );
    if pattern.is_empty() {
        return false;
    }
    let text = if pattern.contains("://") {
        url.as_str()
    } else {
        url.split_once("://").map_or(url.as_str(), |(_, rest)| rest)
    };
    wildcard(&pattern, text)
}

/// Is a copied link named `name` a file to offer? A copied address is often not meant to be
/// downloaded at all (a picture, a font or a model to paste into code), and it says nothing about
/// what is behind it, unlike a browser that is really downloading. So the clipboard keeps to the
/// list as it stood before #382, plus what the user typed in themselves; the types #382 added are
/// the browser's alone, and a lone `*` (every type) doesn't count.
pub fn clipboard_type(types: &str, name: &str) -> bool {
    let own: Vec<&str> = types.split_whitespace().filter(|t| *t != "*").collect();
    let browser_only =
        type_listed(DEFAULT_AUTO_TYPES, name) && !type_listed(EARLIER_AUTO_TYPES[2], name);
    !browser_only && type_listed(&own.join(" "), name)
}

/// `*` matches any run of characters (IDM's address patterns).
fn wildcard(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let mut rest = text;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            match rest.strip_prefix(part) {
                Some(r) => rest = r,
                None => return false,
            }
        } else if i == parts.len() - 1 {
            return rest.ends_with(part);
        } else {
            match rest.find(part) {
                Some(at) => rest = &rest[at + part.len()..],
                None => return false,
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHROME_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";
    const EDGE_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36 Edg/141.0.0.0";
    const FIREFOX_UA: &str =
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:145.0) Gecko/20100101 Firefox/145.0";
    const CHROME: &str = "Google Chrome";
    const EDGE: &str = "Microsoft Edge";
    const FIREFOX: &str = "Mozilla Firefox";

    /// #424: the list of sites as a file and back: one a line, duplicates (whatever their case)
    /// not added, what isn't a site left out.
    #[test]
    fn sites_export_and_import() {
        assert_eq!(sites_to_text("a.test *.b.test"), "a.test\n*.b.test\n");
        assert_eq!(sites_to_text(""), "");
        let (merged, added) = merge_sites(
            "a.test b.test",
            "A.TEST\r\nc.test  d.test\n*.e.test\nbad<name\n",
        );
        assert_eq!(merged, "a.test b.test c.test d.test *.e.test");
        assert_eq!(added, 3);
        assert_eq!(merge_sites("a.test", "").0, "a.test");
    }

    #[test]
    fn browsers_by_name() {
        assert_eq!(browser_named(CHROME_UA, &[]), CHROME);
        assert_eq!(browser_named(EDGE_UA, &[]), EDGE);
        assert_eq!(browser_named(FIREFOX_UA, &[]), FIREFOX);
        let brands = |b: &[&str]| b.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            browser_named(CHROME_UA, &brands(&["Not.A/Brand", "Chromium", "Brave"])),
            "Brave Browser"
        );
        assert_eq!(
            browser_named(CHROME_UA, &brands(&["Chromium", "Google Chrome"])),
            CHROME
        );
        assert_eq!(browser_named(CHROME_UA, &brands(&["Opera"])), "Opera");
        // #41: Firefox's forks by runtime.getBrowserInfo's name; Firefox itself by its UA.
        assert_eq!(browser_named(FIREFOX_UA, &brands(&["Zen"])), "Zen Browser");
        assert_eq!(
            browser_named(FIREFOX_UA, &brands(&["LibreWolf"])),
            "LibreWolf"
        );
        assert_eq!(browser_named(FIREFOX_UA, &brands(&["Firefox"])), FIREFOX);
        // Helium's brands are Chrome's (spoofed on purpose).
        assert_eq!(
            browser_named(
                CHROME_UA,
                &brands(&["Chromium", "Google Chrome", "Not.A/Brand"])
            ),
            CHROME
        );
    }

    #[test]
    fn the_video_button_follows_its_options() {
        let mut r = CaptureRules {
            video_panel_sites: "*.example.com tube.test".into(),
            ..CaptureRules::default()
        };
        assert!(r.panel_on("https://videos.site/watch?v=1", None, CHROME));
        assert!(
            !r.panel_on("https://www.example.com/v", None, CHROME),
            "a listed site"
        );
        assert!(
            !r.panel_on("https://example.com/v", None, CHROME),
            "*. takes the domain too"
        );
        assert!(!r.panel_on("https://TUBE.test/v", None, FIREFOX));
        r.browsers = vec![("Mozilla Firefox".into(), false)];
        assert!(
            !r.panel_on("https://videos.site/", None, FIREFOX),
            "a browser not captured"
        );
        assert!(r.panel_on("https://videos.site/", None, CHROME));
        r.video_panel = false;
        assert!(!r.panel_on("https://videos.site/", None, CHROME));
        // #532: a name without `*.` takes its subdomains (www. above all); a player embedded
        // from another site follows the tab's site.
        r.video_panel = true;
        r.video_panel_sites = "quiet.test".into();
        assert!(!r.panel_on("https://www.quiet.test/v", None, CHROME));
        assert!(!r.panel_on("https://quiet.test/v", None, CHROME));
        assert!(r.panel_on("https://notquiet.test/v", None, CHROME));
        assert!(!r.panel_on(
            "https://videos.site/embed/1",
            Some("https://www.quiet.test/post"),
            CHROME
        ));
        assert!(r.panel_on(
            "https://videos.site/embed/1",
            Some("https://blog.site/"),
            CHROME
        ));
        assert!(r.panel_on("https://videos.site/embed/1", Some(""), CHROME));
        // #759: wildcard support in download panel blacklist.
        r.video_panel = true;
        r.video_panel_sites = "*.youtube.* www.vimeo.co* *dailymotion*".into();
        assert!(!r.panel_on("https://www.youtube.com/watch?v=1", None, CHROME));
        assert!(!r.panel_on("https://youtube.com/watch?v=1", None, CHROME));
        assert!(!r.panel_on("https://m.youtube.co.uk/watch?v=1", None, CHROME));
        assert!(!r.panel_on("https://www.vimeo.com/123", None, CHROME));
        assert!(!r.panel_on("https://www.vimeo.co/123", None, CHROME));
        assert!(r.panel_on("https://www.vimeo.org/123", None, CHROME));
        assert!(!r.panel_on("https://www.dailymotion.com/video/x", None, CHROME));
        assert!(!r.panel_on("https://sub.dailymotion.fr/video/x", None, CHROME));
        assert!(r.panel_on("https://other-site.com/video", None, CHROME));
        r.video_panel = true;
        r.advanced_integration = false;
        assert!(!r.panel_on("https://videos.site/", None, CHROME));
    }

    #[test]
    fn small_files_by_options() {
        let mut r = CaptureRules::default();
        assert!(!r.too_small(Some(10)), "any size by default");
        r.capture_min_kb = 100;
        assert!(r.too_small(Some(15 * 1024)));
        assert!(!r.too_small(Some(100 * 1024)) && !r.too_small(None) && !r.too_small(Some(0)));
    }

    #[test]
    fn a_site_is_added_once() {
        assert_eq!(add_site("", "Video.Site"), "video.site");
        assert_eq!(add_site("a.test", "b.test"), "a.test b.test");
        assert_eq!(
            add_site("*.b.test", "www.b.test"),
            "*.b.test",
            "already covered"
        );
        assert_eq!(add_site("a.test", " "), "a.test");
        assert_eq!(add_site("a.test", "b.test *"), "a.test", "one host only");
        assert_eq!(add_site("a.test", "*"), "a.test");
        assert_eq!(
            remove_site("a.test Video.Site b.test", "video.site"),
            "a.test b.test"
        );
        assert_eq!(
            remove_site("*.b.test", "www.b.test"),
            "*.b.test",
            "a pattern stays"
        );
        assert_eq!(remove_site("a.test", "c.test"), "a.test");
    }

    #[test]
    fn the_defaults_take_listed_types_from_any_browser() {
        let r = CaptureRules::default();
        assert_eq!(
            r.accepts("https://example.com/a/setup.exe", "setup.exe", CHROME),
            Ok(())
        );
        assert_eq!(
            r.accepts(
                "https://example.com/movie.part.r01",
                "movie.part.r01",
                FIREFOX
            ),
            Ok(())
        );
        assert_eq!(
            r.accepts("https://example.com/page.html", "page.html", CHROME),
            Err(Declined::FileType)
        );
        // #39: a .torrent, by default (Add torrent shows what it holds); a list saved before keeps
        // what it had.
        assert_eq!(
            r.accepts("https://example.com/dl/set.torrent", "set.torrent", CHROME),
            Ok(())
        );
        let saved: CaptureRules = serde_json::from_str(r#"{"auto_types":"ZIP EXE"}"#).unwrap();
        assert_eq!(
            saved.accepts("https://example.com/dl/set.torrent", "set.torrent", CHROME),
            Err(Declined::FileType)
        );
    }

    #[test]
    fn excluded_sites_addresses_and_browsers_are_left_to_the_browser() {
        let mut r = CaptureRules::default();
        assert_eq!(
            r.accepts("https://dl.update.microsoft.com/x.exe", "x.exe", CHROME),
            Err(Declined::Site)
        );
        r.excluded_addresses = vec!["https://example.com/private/*".into()];
        assert_eq!(
            r.accepts("https://example.com/private/a.zip", "a.zip", CHROME),
            Err(Declined::Address)
        );
        assert_eq!(
            r.accepts("https://example.com/public/a.zip", "a.zip", CHROME),
            Ok(())
        );
        r.browsers = vec![
            ("Microsoft Edge".into(), false),
            ("Google Chrome".into(), true),
        ];
        assert_eq!(
            r.accepts("https://example.com/a.zip", "a.zip", EDGE),
            Err(Declined::Browser)
        );
        assert_eq!(
            r.accepts("https://example.com/a.zip", "a.zip", CHROME),
            Ok(())
        );
        r.advanced_integration = false;
        assert_eq!(
            r.accepts("https://example.com/a.zip", "a.zip", CHROME),
            Err(Declined::IntegrationOff)
        );
    }

    #[test]
    fn hosts_and_patterns() {
        assert_eq!(
            host_of("https://user:pw@Example.COM:8080/x?y"),
            "example.com"
        );
        assert_eq!(host_of("http://[::1]:80/"), "::1");
        assert!(host_matches("a.b.example.com", "*.example.com"));
        assert!(host_matches("example.com", "*.example.com"));
        assert!(!host_matches("badexample.com", "*.example.com"));
        assert!(
            !host_matches("othermysite.com", "*mysite.com"),
            "host_matches treats * literally"
        );
        // Wildcard patterns for download panel (#759)
        assert!(panel_site_matches("www.youtube.com", "*.youtube.*"));
        assert!(panel_site_matches("youtube.com", "*.youtube.*"));
        assert!(panel_site_matches("youtube.co", "youtube.*"));
        assert!(panel_site_matches("youtube.com", "youtube.*"));
        assert!(!panel_site_matches("vimeo.com", "youtube.*"));
        assert!(panel_site_matches("www.youtube.com", "www.youtube.co*"));
        assert!(panel_site_matches("www.youtube.co", "www.youtube.co*"));
        assert!(panel_site_matches("www.youtube.co.uk", "www.youtube.co*"));
        assert!(!panel_site_matches("www.youtube.es", "www.youtube.co*"));
        assert!(panel_site_matches("www.youtube.com", "*youtube*"));
        assert!(panel_site_matches("youtube.com", "*youtube*"));
        assert!(!panel_site_matches("vimeo.com", "*youtube*"));
        assert!(wildcard("*/files/*.zip", "https://x.org/files/a.zip"));
        assert!(address_matches(
            "example.com/private/*",
            "https://Example.com/private/a.zip"
        ));
        assert!(address_matches(
            "https://example.com/private/*",
            "https://example.com/private/a"
        ));
        assert!(!address_matches(
            "https://example.com/private/*",
            "http://example.com/private/a"
        ));
        assert!(!address_matches(" ", "https://x.org/"));
        assert!(!wildcard("*/files/*.zip", "https://x.org/files/a.rar"));
    }

    #[test]
    fn an_earlier_default_list_becomes_todays_and_an_edited_one_stays() {
        for old in EARLIER_AUTO_TYPES {
            assert_eq!(upgraded_types(old), Some(DEFAULT_AUTO_TYPES));
            // In another order or case: the same list, never edited.
            let shuffled: Vec<String> = old.split(' ').rev().map(str::to_lowercase).collect();
            assert_eq!(
                upgraded_types(&shuffled.join("  ")),
                Some(DEFAULT_AUTO_TYPES)
            );
        }
        // 0.2.4's list: a .dmg, .deb or .webm is taken once upgraded (#208).
        let r = CaptureRules {
            auto_types: EARLIER_AUTO_TYPES[0].into(),
            ..CaptureRules::default()
        }
        .upgraded();
        for name in [
            "MDM.dmg",
            "mdm_0.3.0_amd64.deb",
            "a.AppImage",
            "clip.webm",
            "x.tar.zst",
        ] {
            assert!(r.auto_type(name), "{name}");
        }
        // Edited by the user: one type more, one less, or their own list: left as it is.
        let more = format!("{} DMG", EARLIER_AUTO_TYPES[0]);
        let less = EARLIER_AUTO_TYPES[1].replace(" ZIP", "");
        for edited in [more.as_str(), less.as_str(), "ZIP RAR", ""] {
            assert_eq!(upgraded_types(edited), None, "{edited:?}");
            let r = CaptureRules {
                auto_types: edited.into(),
                ..CaptureRules::default()
            }
            .upgraded();
            assert_eq!(r.auto_types, edited);
        }
        // Today's needs nothing; it holds every earlier default's types.
        assert_eq!(upgraded_types(DEFAULT_AUTO_TYPES), None);
        for old in EARLIER_AUTO_TYPES {
            for t in old.split(' ') {
                assert!(DEFAULT_AUTO_TYPES.split(' ').any(|n| n == t), "{t}");
            }
        }
    }

    #[test]
    fn todays_big_files_are_taken_and_everyday_browsing_is_not() {
        // #382: an untouched list from before gets them too.
        let earlier = CaptureRules {
            auto_types: EARLIER_AUTO_TYPES[2].into(),
            ..CaptureRules::default()
        };
        assert!(!earlier.auto_type("model.Q4_K_M.gguf"));
        let r = earlier.upgraded();
        for name in [
            "model.Q4_K_M.gguf",
            "model-00001-of-00004.safetensors",
            "sd15.ckpt",
            "weights.pth",
            "model.onnx",
            "train-00000.parquet",
            "scene.glb",
            "part.stl",
            "character.fbx",
            "house.blend",
            "bracket.step",
            "IMG_0001.CR3",
            "cover.psd",
            "book.epub",
            "report.docx",
            "budget.xlsx",
            "disk.vhdx",
            "backup.7z.001",
            "backup.z01",
            "movie.r27",
            "5GB-testfile.dat",
        ] {
            assert!(r.auto_type(name), "{name}");
        }
        for name in [
            "index.html",
            "app.js",
            "style.css",
            "config.json",
            "notes.txt",
            "photo.jpg",
            "logo.png",
            "icon.svg",
            "addon.xpi",
            "addon.crx",
        ] {
            assert!(!r.auto_type(name), "{name}");
        }
    }

    #[test]
    fn a_lone_star_takes_every_file_with_a_type() {
        let r = CaptureRules {
            auto_types: "*".into(),
            ..CaptureRules::default()
        };
        for name in ["model.gguf", "photo.jpg", "anything.xyz123"] {
            assert_eq!(r.accepts("https://x.test/f", name, "Google Chrome"), Ok(()));
        }
        assert!(!r.auto_type("no_extension"));
    }

    #[test]
    fn a_copied_address_is_offered_only_as_it_always_was() {
        // What people copy to paste somewhere else, never a download by default.
        for name in [
            "photo.jpg",
            "logo.png",
            "page.html",
            "font.ttf",
            "scene.glb",
            "model.gguf",
            "a.safetensors",
            "jane.ma",
            "site.ai",
            "cover.psd",
            "winmail.dat",
        ] {
            assert!(!clipboard_type(DEFAULT_AUTO_TYPES, name), "{name}");
            assert!(!clipboard_type("*", name), "{name}, a lone star");
            assert!(!clipboard_type("* ZIP", name), "{name}, a star and more");
        }
        // Every type offered before #382 still is.
        for t in EARLIER_AUTO_TYPES[2].split(' ') {
            let name = format!("a.{}", t.replace('*', "1"));
            assert!(clipboard_type(DEFAULT_AUTO_TYPES, &name), "{name}");
        }
        // A type the user listed themselves; and one they took off.
        assert!(clipboard_type("ZIP XYZ", "a.xyz"));
        assert!(!clipboard_type("ZIP", "a.exe"));
    }

    #[test]
    fn each_refusal_has_a_name_and_a_reason() {
        for d in Declined::ALL {
            assert_eq!(Declined::from_code(d.code()), Some(d));
            assert!(d.reason().contains("Options >"), "{d:?}");
        }
        assert_eq!(Declined::from_code("FileType"), None);
        let mut r = CaptureRules {
            browsers: vec![(EDGE.into(), false)],
            ..CaptureRules::default()
        };
        assert_eq!(r.capture_off(CHROME), None);
        assert_eq!(r.capture_off(EDGE), Some(Declined::Browser));
        r.advanced_integration = false;
        assert_eq!(r.capture_off(CHROME), Some(Declined::IntegrationOff));
        assert!(!r.captures(CHROME));
    }
}
