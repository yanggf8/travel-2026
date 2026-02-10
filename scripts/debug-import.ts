#!/usr/bin/env ts-node
import { parseScraperOfferAsOfferRow, toInsertSql } from './import-offers-to-turso';
import fs from 'node:fs';
import path from 'node:path';

const dataPath = path.resolve('data/trips/osaka-kyoto-2026/osaka_kyoto_2026-packages-scrape.json');
const data = JSON.parse(fs.readFileSync(dataPath, 'utf-8'));
const offer = data.offers[0];

console.log('Input offer:', JSON.stringify(offer, null, 2));
console.log('\n' + '='.repeat(80) + '\n');

const row = parseScraperOfferAsOfferRow('test.json', offer, 'osaka_kyoto_2026', 'kansai');
console.log('Parsed row:', JSON.stringify(row, null, 2));
console.log('\n' + '='.repeat(80) + '\n');

if (row) {
  console.log('SQL:', toInsertSql(row));
}
