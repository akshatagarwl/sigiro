import type { Metadata } from "next";
import { Geist, Geist_Mono } from "next/font/google";

import "./globals.css";
import { AppShell } from "@/components/sigiro/app-shell";
import { ThemeProvider } from "@/components/theme-provider";

const fontSans = Geist({
  subsets: ["latin"],
  variable: "--font-sans",
});

const fontMono = Geist_Mono({
  subsets: ["latin"],
  variable: "--font-mono",
});

export const metadata: Metadata = {
  title: "Dashboard · Sigiro",
  description: "Services, traces and logs for your Sigiro workspace.",
  robots: { index: false, follow: false },
};

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode;
}>) {
  return (
    <html
      lang="en"
      suppressHydrationWarning
      className={`${fontSans.variable} ${fontMono.variable} font-sans antialiased`}
    >
      <body>
        {/* Same storage key as sigiro.com, so the site and the dashboard share one theme choice. */}
        <ThemeProvider storageKey="blume-theme">
          <AppShell>{children}</AppShell>
        </ThemeProvider>
      </body>
    </html>
  );
}
