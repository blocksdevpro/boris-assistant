import { ArrowRight } from "lucide-react";
import Link from "next/link";
import { betaRelease, stableRelease } from "@/lib/releases";
import { pageMetadata, repo } from "@/lib/site";

export const metadata = pageMetadata({
  title: "Boris Assistant release notes | Stable and beta updates",
  description: `See what's new in Boris Assistant ${betaRelease.version} and stable ${stableRelease.version}: local memory, faster voice replies, background research, and Windows downloads.`,
  path: "/releases",
});

export default function ReleasesPage() {
  return (
    <main id="main-content" className="content-page section-shell">
      <p className="story-label"><span /> Published updates</p>
      <h1>Boris Assistant release notes</h1>
      <p className="page-intro">The latest published release is beta {betaRelease.version}. For everyday use, the current stable release is {stableRelease.version}.</p>

      {[betaRelease, stableRelease].map((release) => (
        <article key={release.version} id={release.channel.toLowerCase()} className="release-entry">
          <div className="release-date">
            <span className={`channel-label ${release.channel === "Beta" ? "beta-label" : ""}`}>{release.channel}</span>
            <time dateTime={release.date}>{release.dateLabel}</time>
          </div>
          <div>
            <h2>Boris {release.version}</h2>
            <p className="release-lead">{release.summary}</p>
            <ul>{release.highlights.map((highlight) => <li key={highlight}>{highlight}</li>)}</ul>
            {release.channel === "Beta" && (
              <aside className="migration-note">
                <h3>Memory migration and privacy</h3>
                <p>The one-time migration sends historical memory to the LLM provider already configured for Boris. Old files are retired only after refinement and verification succeed. Failed imports leave the original files intact for a later retry.</p>
                <p>To disable canonical memory and skip migration, set <code>BORIS_MEMORY=0</code> before the first beta.2 launch.</p>
              </aside>
            )}
            <div className="release-links">
              <Link href={release.channel === "Beta" ? "/download#beta" : "/download"} className="text-link">Download {release.channel.toLowerCase()} <ArrowRight size={16} /></Link>
              <a href={release.url} className="text-link">Full notes on GitHub <ArrowRight size={16} /></a>
            </div>
          </div>
        </article>
      ))}
      <a href={`${repo}/releases`} className="text-link">All published releases on GitHub <ArrowRight size={16} /></a>
    </main>
  );
}
