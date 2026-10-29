#!/usr/bin/env node
// Prints the Docker daemon settings that send Docker Hub pulls through a mirror (09/T2; S19, S20).
//
//   node .github/scripts/docker-hub-mirror.mjs <mirror-host> [/etc/docker/daemon.json]  >  new-daemon.json
//
// Docker Hub limits anonymous pulls per IP address, and GitHub-hosted runners share addresses, so a run can get
// "429 Too Many Requests". A mirror such as mirror.gcr.io serves the same content. Every image in this repository is named by
// digest, so the mirror needs no trust: content that does not match the digest is refused, and a mirror that fails falls back to
// Docker Hub. The existing settings are kept; the mirror is put first and never listed twice. The docker-hub-mirror action
// writes the result and restarts the daemon. Exit 2 on a host that is not a plain host name, or a daemon.json it cannot extend.
import { existsSync, readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

const HOST = /^[a-z0-9]([a-z0-9.-]*[a-z0-9])?(:[0-9]{1,5})?$/;

export function mergeMirror(existingText, host) {
  if (!HOST.test(host) || (host.includes(':') && Number(host.split(':')[1]) > 65535)) {
    throw new Error(`the mirror host ${JSON.stringify(host)} is not a plain host name (with an optional port)`);
  }
  let config = {};
  if (existingText.trim() !== '') {
    try {
      config = JSON.parse(existingText);
    } catch {
      throw new Error('daemon.json is not valid JSON; refusing to overwrite it');
    }
    if (config === null || typeof config !== 'object' || Array.isArray(config)) {
      throw new Error('daemon.json is not a JSON object; refusing to overwrite it');
    }
  }
  const url = `https://${host}`;
  const current = config['registry-mirrors'] ?? [];
  if (!Array.isArray(current) || current.some((m) => typeof m !== 'string')) {
    throw new Error('registry-mirrors in daemon.json is not a list of strings; refusing to overwrite it');
  }
  return { ...config, 'registry-mirrors': [url, ...current.filter((m) => m !== url)] };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [host = '', path = '/etc/docker/daemon.json'] = process.argv.slice(2);
  try {
    const text = existsSync(path) ? readFileSync(path, 'utf8') : '';
    process.stdout.write(`${JSON.stringify(mergeMirror(text, host), null, 2)}\n`);
  } catch (e) {
    console.error(`docker-hub-mirror: ${e.message}`);
    process.exitCode = 2;
  }
}
