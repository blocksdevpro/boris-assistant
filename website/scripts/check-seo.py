"""Check the rendered website's crawl and sharing metadata using Python 3."""

import argparse
import json
import struct
from html.parser import HTMLParser
from urllib.error import HTTPError
from urllib.parse import urljoin, urlsplit, urlunsplit
from urllib.request import Request, urlopen
from xml.etree import ElementTree


class Page(HTMLParser):
    def __init__(self, html):
        super().__init__()
        self.title = ""
        self.h1_count = 0
        self.h2s = []
        self.in_h2 = False
        self.meta = {}
        self.canonicals = []
        self.links = []
        self.ids = set()
        self.schemas = []
        self.in_title = False
        self.in_schema = False
        self.schema_text = ""
        self.feed(html)

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if attrs.get("id"):
            self.ids.add(attrs["id"])
        if tag == "title":
            self.in_title = True
        elif tag == "h1":
            self.h1_count += 1
        elif tag == "h2":
            self.h2s.append("")
            self.in_h2 = True
        elif tag == "meta":
            self.meta[attrs.get("name", attrs.get("property"))] = attrs.get("content", "")
        elif tag == "link" and attrs.get("rel") == "canonical":
            self.canonicals.append(attrs.get("href"))
        elif tag == "a" and attrs.get("href"):
            self.links.append(attrs["href"])
        elif tag == "script" and attrs.get("type") == "application/ld+json":
            self.in_schema = True
            self.schema_text = ""

    def handle_data(self, data):
        if self.in_title:
            self.title += data
        if self.in_h2:
            self.h2s[-1] += data
        if self.in_schema:
            self.schema_text += data

    def handle_endtag(self, tag):
        if tag == "title":
            self.in_title = False
        elif tag == "h2":
            self.in_h2 = False
        elif tag == "script" and self.in_schema:
            self.schemas.append(json.loads(self.schema_text))
            self.in_schema = False


def check(base, origin, stable_version=None, beta_version=None):
    def normalized_url(url):
        parts = urlsplit(url)
        return urlunsplit((parts.scheme, parts.netloc, parts.path or "/", parts.query, parts.fragment))

    def fetch(path):
        request = Request(urljoin(base, path), headers={"User-Agent": "Boris-SEO-check/1.0"})
        with urlopen(request, timeout=30) as response:
            assert response.status == 200, f"{path}: expected HTTP 200"
            return response.headers, response.read()

    pages = {}
    images = set()
    for path in ("/", "/download", "/releases"):
        headers, body = fetch(path)
        assert "text/html" in headers.get("Content-Type", ""), f"{path}: not HTML"
        assert "noindex" not in headers.get("X-Robots-Tag", "").lower(), f"{path}: blocked by header"
        page = Page(body.decode("utf-8"))
        canonical = urljoin(origin, path)
        assert page.h1_count == 1, f"{path}: expected one H1"
        assert [normalized_url(url) for url in page.canonicals] == [canonical], f"{path}: wrong or duplicate canonical"
        assert page.title and page.meta.get("description"), f"{path}: missing search copy"
        for directive in ("robots", "googlebot"):
            assert "noindex" not in page.meta.get(directive, "").lower(), f"{path}: blocked by {directive}"
        assert normalized_url(page.meta.get("og:url", "")) == canonical, f"{path}: wrong Open Graph URL"
        assert page.meta.get("og:site_name") == "Boris Assistant", f"{path}: missing site name"
        assert page.meta.get("twitter:card") == "summary_large_image", f"{path}: wrong social card"
        for key in ("og:image", "twitter:image"):
            image = urlsplit(page.meta.get(key, ""))
            assert image.scheme == "https" and image.netloc == urlsplit(origin).netloc, f"{path}: nonproduction {key}"
            images.add(urlunsplit(("", "", image.path, image.query, "")))
        pages[path] = page
        print(f"PASS {path}: title, description, canonical, H1, crawlability, sharing metadata")

    assert len({page.title for page in pages.values()}) == len(pages), "Duplicate titles"
    assert len({page.meta["description"] for page in pages.values()}) == len(pages), "Duplicate descriptions"
    for path, page in pages.items():
        for href in page.links:
            target = urlsplit(urljoin(origin + path, href))
            if target.netloc != urlsplit(origin).netloc:
                continue
            assert target.path in pages, f"{path}: unexpected internal page {href}"
            assert not target.fragment or target.fragment in pages[target.path].ids, f"{path}: broken anchor {href}"
    print("PASS internal page links and section anchors")

    _, robots = fetch("/robots.txt")
    assert f"Sitemap: {origin}/sitemap.xml" in robots.decode(), "Wrong robots sitemap"
    assert "Allow: /" in robots.decode() and "Disallow: /" not in robots.decode(), "Public crawling blocked"
    _, sitemap = fetch("/sitemap.xml")
    locations = ElementTree.fromstring(sitemap).findall("{*}url/{*}loc")
    assert {item.text for item in locations} == {urljoin(origin, path) for path in pages}, "Sitemap mismatch"
    print("PASS robots.txt and sitemap.xml")

    graph = pages["/"].schemas[0]["@graph"]
    website = next(item for item in graph if item["@type"] == "WebSite")
    application = next(item for item in graph if item["@type"] == "SoftwareApplication")
    assert website["url"] == origin + "/" and website["name"] == "Boris Assistant", "Wrong site identity"
    assert application["downloadUrl"] in pages["/download"].links, "Schema download differs from page"
    assert f'/v{application["softwareVersion"]}/' in application["downloadUrl"], "Schema version differs from download"
    assert application["offers"]["price"] == "0", "App price is wrong"
    assert "aggregateRating" not in application and "review" not in application, "Unverified review data"
    print("PASS structured data and stable download consistency")

    if stable_version:
        for path in ("/download", "/releases"):
            assert f"Boris {stable_version}" in pages[path].h2s, f"{path}: wrong stable heading"
        repo = "https://github.com/blocksdevpro/boris-assistant"
        release = f"{repo}/releases/download/v{stable_version}"
        assert f"{release}/Boris_{stable_version}_x64-setup.exe" in pages["/download"].links, "Stable EXE does not match the release"
        assert f"{release}/Boris_{stable_version}_x64_en-US.msi" in pages["/download"].links, "Stable MSI does not match the release"
        assert f"{repo}/releases/tag/v{stable_version}" in pages["/releases"].links, "Wrong stable release link"
        assert application["softwareVersion"] == stable_version, "Schema stable version is wrong"
        print(f"PASS published stable {stable_version}: headings, installers, release link, and schema")

    if beta_version:
        for path in ("/download", "/releases"):
            assert f"Boris {beta_version}" in pages[path].h2s, f"{path}: wrong beta heading"
        repo = "https://github.com/blocksdevpro/boris-assistant"
        asset = f"{repo}/releases/download/v{beta_version}/Boris_{beta_version}_x64-setup.exe"
        assert asset in pages["/download"].links, "Beta installer does not match the release"
        assert f"{repo}/releases/tag/v{beta_version}" in pages["/releases"].links, "Wrong beta release link"
        print(f"PASS published beta {beta_version}: headings, installer, and release link")

    for path in images:
        headers, data = fetch(path)
        assert "image/png" in headers.get("Content-Type", ""), f"{path}: not a PNG"
        assert data[:8] == b"\x89PNG\r\n\x1a\n", f"{path}: invalid PNG"
        assert struct.unpack(">II", data[16:24]) == (1200, 630), f"{path}: wrong dimensions"
    print("PASS social images resolve to 1200x630 PNGs")

    try:
        fetch("/seo-check-missing-page")
    except HTTPError as error:
        assert error.code == 404, f"Missing page returned {error.code}"
    else:
        raise AssertionError("Missing page must return HTTP 404")
    print("PASS missing page returns HTTP 404")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("base", nargs="?", default="http://127.0.0.1:3000")
    parser.add_argument("--origin", default="https://boris.blocksdev.pro")
    parser.add_argument("--stable-version", help="Expected published stable version")
    parser.add_argument("--beta-version", help="Expected published beta version")
    args = parser.parse_args()
    check(args.base.rstrip("/"), args.origin.rstrip("/"), args.stable_version, args.beta_version)
