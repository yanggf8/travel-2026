/**
 * Wildcard Expansion for Cascade Rules
 *
 * Mode: schema_driven
 * Expands wildcards against schema_contract.process_nodes (process roots only)
 */

import { SchemaContract } from './types';

/**
 * Expand a pattern like "process_3_*" against process_nodes.
 *
 * @param pattern - Pattern with optional wildcard (e.g., "process_3_*")
 * @param processNodes - Authoritative list from schema_contract.process_nodes
 * @returns Expanded list of matching process names, sorted alphabetically
 */
export function expandWildcard(pattern: string, processNodes: string[]): string[] {
  // No wildcard - return as-is (if it exists in process_nodes or is a valid path)
  if (!pattern.includes('*')) {
    return [pattern];
  }

  // Extract prefix before wildcard
  const prefix = pattern.replace('*', '');

  // Match against process_nodes
  const matches = processNodes
    .filter(node => node.startsWith(prefix))
    .sort(); // Deterministic order

  return matches;
}

/**
 * Expand multiple patterns, deduplicating and sorting results.
 *
 * @param patterns - Array of patterns (may include wildcards)
 * @param processNodes - Authoritative list from schema_contract.process_nodes
 * @returns Deduplicated, sorted list of process names
 */
export function expandPatterns(patterns: string[], processNodes: string[]): string[] {
  const expanded = new Set<string>();

  for (const pattern of patterns) {
    const matches = expandWildcard(pattern, processNodes);
    for (const match of matches) {
      expanded.add(match);
    }
  }

  return Array.from(expanded).sort();
}

/**
 * Validate that all expanded targets exist in process_nodes.
 *
 * @param targets - Expanded target list
 * @param processNodes - Authoritative list
 * @returns Array of invalid targets (empty if all valid)
 */
export function validateTargets(targets: string[], processNodes: string[]): string[] {
  const nodeSet = new Set(processNodes);
  return targets.filter(t => !nodeSet.has(t));
}

/**
 * Get the expansion rules documentation for a schema contract.
 */
export function getExpansionDocs(schemaContract: SchemaContract): Record<string, string[]> {
  const processNodes = schemaContract.process_nodes;

  return {
    'process_*': expandWildcard('process_*', processNodes),
    'process_2_*': expandWildcard('process_2_*', processNodes),
    'process_3_*': expandWildcard('process_3_*', processNodes),
    'process_4_*': expandWildcard('process_4_*', processNodes),
    'process_5_*': expandWildcard('process_5_*', processNodes),
  };
}
