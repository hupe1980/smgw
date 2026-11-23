"""Check the assembled site, including links into the generated Rust API."""
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import unquote, urljoin, urlsplit
from xml.etree import ElementTree
import tomllib


class Page(HTMLParser):
    def __init__(self, path):
        super().__init__()
        self.ids = set()
        self.refs = []
        self.canonical = None
        self.description = None
        self.feed(path.read_text(encoding="utf-8"))

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if "id" in attrs:
            self.ids.add(attrs["id"])
        if tag == "meta" and attrs.get("name") == "description":
            self.description = attrs.get("content")
        if tag == "link" and attrs.get("rel") == "canonical":
            self.canonical = attrs.get("href")
        for attribute in ("href", "src"):
            if attrs.get(attribute):
                self.refs.append(attrs[attribute])


def main():
    root = Path(__file__).resolve().parents[1]
    output = root / "target/site"
    base = tomllib.loads((root / "site/config.toml").read_text())["base_url"].rstrip("/") + "/"
    sitemap = ElementTree.parse(output / "sitemap.xml")
    indexed = {node.text for node in sitemap.findall(".//{*}loc")}
    seen = {}

    def read(path):
        if path not in seen:
            seen[path] = Page(path)
        return seen[path]

    for route in ("", "guide/"):
        path = output / route / "index.html"
        page = read(path)
        url = base + route
        if url not in indexed or page.canonical != url or not page.description:
            raise SystemExit(f"{path}: check sitemap, canonical URL and description")
        for ref in page.refs:
            resolved = urlsplit(urljoin(url, ref))
            if resolved.netloc != urlsplit(base).netloc:
                continue
            prefix = urlsplit(base).path
            if not resolved.path.startswith(prefix):
                raise SystemExit(f"{path}: link leaves project base: {ref}")
            target = output / unquote(resolved.path[len(prefix):])
            if target.is_dir():
                target /= "index.html"
            if not target.is_file():
                raise SystemExit(f"{path}: missing target: {ref}")
            if resolved.fragment and unquote(resolved.fragment) not in read(target).ids:
                raise SystemExit(f"{path}: missing anchor: {ref}")
    print("Landing page, guide, sitemap and local API links verified.")


if __name__ == "__main__":
    main()
