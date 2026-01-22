/**
 * Rule Evaluator for travel-plan.json readiness_rules
 * Supports: present, min_length, for_each with all/any combinators
 */

export interface Rule {
  type: 'present' | 'min_length' | 'for_each';
  path: string;
  min?: number;
  all?: Rule[];
  any?: Rule[];
}

export interface RuleBlock {
  all?: Rule[];
  any?: Rule[];
}

export interface EvalResult {
  passed: boolean;
  missingFields: string[];
  details: string[];
}

/**
 * Get value at dot-notation path from object
 */
function getPath(obj: any, path: string): any {
  const parts = path.split('.');
  let current = obj;
  for (const part of parts) {
    if (current === null || current === undefined) {
      return undefined;
    }
    current = current[part];
  }
  return current;
}

/**
 * Check if a value is "present" (non-null, non-undefined, non-empty string)
 */
function isPresent(value: any): boolean {
  if (value === null || value === undefined) return false;
  if (typeof value === 'string' && value.trim() === '') return false;
  return true;
}

/**
 * Evaluate a single rule against data
 */
export function evaluateRule(
  rule: Rule,
  data: any,
  contextPath: string = ''
): EvalResult {
  const fullPath = contextPath ? `${contextPath}.${rule.path}` : rule.path;

  switch (rule.type) {
    case 'present': {
      const value = getPath(data, rule.path);
      const passed = isPresent(value);
      return {
        passed,
        missingFields: passed ? [] : [fullPath],
        details: passed ? [] : [`Missing: ${fullPath}`],
      };
    }

    case 'min_length': {
      const value = getPath(data, rule.path);
      const length = Array.isArray(value) ? value.length : 0;
      const passed = length >= (rule.min || 0);
      return {
        passed,
        missingFields: passed ? [] : [fullPath],
        details: passed
          ? []
          : [`${fullPath} has ${length} items, need >= ${rule.min}`],
      };
    }

    case 'for_each': {
      const array = getPath(data, rule.path);
      if (!Array.isArray(array)) {
        return {
          passed: false,
          missingFields: [fullPath],
          details: [`${fullPath} is not an array`],
        };
      }

      const allMissing: string[] = [];
      const allDetails: string[] = [];
      let allPassed = true;

      for (let i = 0; i < array.length; i++) {
        const item = array[i];
        const itemPath = `${fullPath}[${i}]`;

        // Evaluate nested all/any blocks
        if (rule.all) {
          const result = evaluateAllBlock(rule.all, item, itemPath);
          if (!result.passed) allPassed = false;
          allMissing.push(...result.missingFields);
          allDetails.push(...result.details);
        }

        if (rule.any) {
          const result = evaluateAnyBlock(rule.any, item, itemPath);
          if (!result.passed) allPassed = false;
          allMissing.push(...result.missingFields);
          allDetails.push(...result.details);
        }
      }

      return {
        passed: allPassed,
        missingFields: allMissing,
        details: allDetails,
      };
    }

    default:
      return {
        passed: false,
        missingFields: [],
        details: [`Unknown rule type: ${(rule as any).type}`],
      };
  }
}

/**
 * Evaluate an "all" block - all rules must pass
 */
export function evaluateAllBlock(
  rules: Rule[],
  data: any,
  contextPath: string = ''
): EvalResult {
  const allMissing: string[] = [];
  const allDetails: string[] = [];
  let allPassed = true;

  for (const rule of rules) {
    const result = evaluateRule(rule, data, contextPath);
    if (!result.passed) allPassed = false;
    allMissing.push(...result.missingFields);
    allDetails.push(...result.details);
  }

  return {
    passed: allPassed,
    missingFields: allMissing,
    details: allDetails,
  };
}

/**
 * Evaluate an "any" block - at least one rule must pass
 */
export function evaluateAnyBlock(
  rules: Rule[],
  data: any,
  contextPath: string = ''
): EvalResult {
  const allMissing: string[] = [];
  const allDetails: string[] = [];
  let anyPassed = false;

  for (const rule of rules) {
    const result = evaluateRule(rule, data, contextPath);
    if (result.passed) anyPassed = true;
    allMissing.push(...result.missingFields);
    allDetails.push(...result.details);
  }

  return {
    passed: anyPassed,
    // If any passed, clear missing fields (they're optional in "any" context)
    missingFields: anyPassed ? [] : allMissing,
    details: anyPassed ? [] : allDetails,
  };
}

/**
 * Evaluate a rule block (top-level all/any)
 * Fails closed: empty or malformed blocks return passed: false
 */
export function evaluateRuleBlock(
  block: RuleBlock | null | undefined,
  data: any,
  contextPath: string = ''
): EvalResult {
  // Fail closed on null/undefined block
  if (!block) {
    return {
      passed: false,
      missingFields: [],
      details: ['Rule block is null or undefined'],
    };
  }

  if (block.all) {
    // Fail closed on empty all array
    if (block.all.length === 0) {
      return {
        passed: false,
        missingFields: [],
        details: ['Rule block has empty "all" array'],
      };
    }
    return evaluateAllBlock(block.all, data, contextPath);
  }

  if (block.any) {
    // Fail closed on empty any array
    if (block.any.length === 0) {
      return {
        passed: false,
        missingFields: [],
        details: ['Rule block has empty "any" array'],
      };
    }
    return evaluateAnyBlock(block.any, data, contextPath);
  }

  // Fail closed: block exists but has neither all nor any
  return {
    passed: false,
    missingFields: [],
    details: ['Rule block has neither "all" nor "any" defined'],
  };
}
