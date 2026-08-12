"use client";

import { useEffect, useState } from "react";
import { ArrowUpRight, Star } from "lucide-react";
import Image from "next/image";

const repo = "https://github.com/blocksdevpro/boris-assistant";

function Mark() {
  return <Image src="/boris-mark.svg" alt="" width={24} height={24} className="header-mark" aria-hidden="true" />;
}

export function MarketingHeader() {
  const [scrolled, setScrolled] = useState(false);

  useEffect(() => {
    const update = () => setScrolled(window.scrollY > 18);
    update();
    window.addEventListener("scroll", update, { passive: true });
    return () => window.removeEventListener("scroll", update);
  }, []);

  return (
    <header className={`marketing-header ${scrolled ? "header-scrolled" : ""}`}>
      <nav className="marketing-nav" aria-label="Primary navigation">
        <a href="#top" className="header-brand"><Mark /><span>Boris</span></a>
        <a href={repo} target="_blank" rel="noreferrer" className="github-pill">
          <Star className="github-star" fill="currentColor" />
          <span><strong>Star on GitHub</strong><small>Open source</small></span>
          <ArrowUpRight className="github-arrow" />
        </a>
      </nav>
    </header>
  );
}
