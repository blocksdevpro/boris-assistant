# Boris website

The public Next.js site lives at https://boris.blocksdev.pro. Run commands from `website/`.

## Run locally

```sh
npm ci
npm run dev
```

Open http://localhost:3000. Use `npm run dev -- --port 3001` if that port is busy.
Before publishing, run `npm run lint` and `npm run build`.
With the production server running, use `python scripts/check-seo.py` to check
rendered metadata, internal links, structured data, crawl files, and image URLs.
Pass a different server URL as its first argument when needed.

## Update released content

1. Check the [published GitHub releases](https://github.com/blocksdevpro/boris-assistant/releases).
2. Update `src/lib/releases.ts` with stable and beta versions, publication dates, exact asset URLs, and release highlights.
3. Update version-specific copy in the home, download, and release pages and their descriptions. Keep pre-release claims labeled as beta.
4. Review memory migration and privacy notes against the tagged release.
5. Run `python scripts/check-seo.py --stable-version 1.2.0 --beta-version 1.2.0-beta.3`
   against the running site to verify displayed versions, installers, and release links.

The website intentionally does not read the working-tree `Unreleased` changelog.
Release data is pinned so an unpublished change cannot silently become a product
claim. Downloads use versioned assets so the installer matches the displayed
version. `SoftwareApplication` metadata describes the default stable download.

## Verify search metadata

`src/lib/site.ts` supplies the production origin, descriptions, canonical URLs,
and sharing metadata. `NEXT_PUBLIC_SITE_URL` can override the origin for a real
domain change. Do not set it to localhost or a preview hostname for production.

Check these routes in the running production build:

- `/`, `/download`, `/releases`: unique titles, descriptions, one H1, and a self-referencing production canonical URL.
- `/robots.txt`: public crawling allowed and the production sitemap URL present.
- `/sitemap.xml`: all three public page URLs, without fragments or preview hosts.
- `/opengraph-image`: a 1200 by 630 PNG with the Boris brand and app screenshot.
- A missing page: HTTP 404, not a successful blank page.

Home renders `WebSite` and `SoftwareApplication` JSON-LD. No ratings or reviews
are invented. App markup describes the product; it does not promise eligibility
for Google's software-app rich results, which have additional requirements.
FAQs use ordinary visible HTML without claiming FAQ rich-result eligibility.

## After deployment

1. Confirm the live routes above and check that the host adds no `noindex` or `X-Robots-Tag: noindex` restriction to production pages.
2. Submit `https://boris.blocksdev.pro/sitemap.xml` in the existing Search Console property. Inspect the home, download, and release URLs, then request indexing.
3. Check Page indexing for exclusions and Google's chosen canonical URL.
4. Compare Search Console queries and pages over 28-day periods. Record branded queries separately from searches such as "open source voice assistant for Windows". Four impressions are too few to judge CTR or broad ranking.
5. Check Core Web Vitals when field data is available. Link to the site from the GitHub repository description and relevant project documentation to help people discover it. Add further guides only when they answer real user needs.

Search results can take days or weeks to reflect changes. Technical fixes help
crawling and interpretation; they cannot guarantee impressions or rankings.

## Research behind these changes

- [Google: useful titles](https://developers.google.com/search/docs/appearance/title-link)
- [Google: snippets and descriptions](https://developers.google.com/search/docs/appearance/snippet)
- [Google: site names](https://developers.google.com/search/docs/appearance/site-names)
- [Google: sitemap guidance](https://developers.google.com/search/docs/crawling-indexing/sitemaps/build-sitemap)
- [Google: helpful content](https://developers.google.com/search/docs/fundamentals/creating-helpful-content)
- [Google: software app structured data](https://developers.google.com/search/docs/appearance/structured-data/software-app)
- [Search Console performance report](https://support.google.com/webmasters/answer/7576553)
