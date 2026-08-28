# Zaaheen site — deploy runbook

NOTHING HERE IS DEPLOYED YET. These are the exact commands, held until asked.

## 1. Create the Pages project (once)

    npx wrangler pages project create zaaheen --production-branch main

## 2. Deploy the site

    npx wrangler pages deploy site --project-name zaaheen

First deploy returns a `https://zaaheen-<hash>.pages.dev` URL. That URL works
immediately and needs no DNS, so the page can be reviewed live before
zaaheen.com points anywhere near it.

## 3. Attach the domain (only after the zone is ACTIVE)

    # zaaheen.com + www -> the Pages project
    npx wrangler pages domain add zaaheen zaaheen.com --project-name zaaheen

Cloudflare replaces the imported Hostinger parking records (A 2.57.91.91 and
CNAME www) at this point. Until then the domain keeps showing Hostinger's
parking page rather than going dark.

## 4. Attach dl.zaaheen.com to the R2 bucket

Dashboard: R2 > zaaheen-releases > Settings > Custom Domains > dl.zaaheen.com

Needs the zone active. This is what makes the installer download global and
zero-egress; without it the only public URL is the dev-only r2.dev one, which
Cloudflare rate-limits and tells you not to use in production.

## 5. Upload the installer

    npx wrangler r2 object put \
      zaaheen-releases/Zaaheen_0.2.0_x64_en-US.msi \
      --file "C:/Projects/MemoryVault-artifacts/Zaaheen_0.2.0_beta-candidate.msi" \
      --content-type application/x-msi --remote

The filename must match the href in site/index.html (`#win-dl` data-href).

## Still to build

- The notify form has NO ENDPOINT. It currently acknowledges in the browser and
  discards the address. Wire it to a Worker + KV/D1 before showing the page to
  anyone, or remove the form — collecting an address and dropping it is worse
  than not asking.
- No macOS or Linux build exists yet.
