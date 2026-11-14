import type { Metadata } from "next";
import React from "react";

export const metadata: Metadata = {
  title: "Subsumptive Registry",
  description: "Package registry for Subsumptive Blocks — Intelligence, unbundled.",
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <body
        style={{
          margin: 0,
          backgroundColor: "#FAF8F5",
          color: "#1C1917",
          fontFamily: "system-ui, -apple-system, sans-serif",
          lineHeight: 1.6,
        }}
      >
        <header
          style={{
            borderBottom: "1px solid #E7E5E4",
            padding: "16px 32px",
            display: "flex",
            justifyContent: "space-between",
            alignItems: "center",
          }}
        >
          <a
            href="/"
            style={{
              textDecoration: "none",
              color: "#1C1917",
              fontWeight: 600,
              letterSpacing: "-0.02em",
            }}
          >
            Subsumptive Registry
          </a>
          <nav style={{ display: "flex", gap: "24px", fontSize: "14px" }}>
            <a href="/search" style={{ color: "#44403C", textDecoration: "none" }}>
              Packages
            </a>
            <a href="/docs/publishing" style={{ color: "#44403C", textDecoration: "none" }}>
              Publish
            </a>
            <a href="/login" style={{ color: "#44403C", textDecoration: "none" }}>
              Sign in
            </a>
          </nav>
        </header>
        <main style={{ maxWidth: "960px", margin: "0 auto", padding: "48px 24px" }}>
          {children}
        </main>
      </body>
    </html>
  );
}
