export interface ParsedVariable {
	name: string;
	type: 'string' | 'number' | 'boolean';
	description: string;
	required: boolean;
	defaultValue?: any;
}

export interface ParsedCurlTool {
	name: string;
	description: string;
	method: string;
	url: string;
	headers: Record<string, string>;
	body?: string;
	cookie_expression?: string;
	captures: {
		cookies?: string;
		json_fields?: Record<string, string>;
	};
	variables: ParsedVariable[];
	session_variables: string[];
	input_schema: {
		type: string;
		properties: Record<string, any>;
		required: string[];
	};
}

/**
 * Tokenize a command line string respecting single/double quotes and escaped spaces.
 */
function tokenizeArgs(cmd: string): string[] {
	const tokens: string[] = [];
	let current = '';
	let inSingle = false;
	let inDouble = false;
	let escaped = false;

	for (let i = 0; i < cmd.length; i++) {
		const c = cmd[i];

		if (escaped) {
			current += c;
			escaped = false;
			continue;
		}

		if (c === '\\' && !inSingle) {
			escaped = true;
			continue;
		}

		if (c === "'" && !inDouble) {
			inSingle = !inSingle;
			continue;
		}

		if (c === '"' && !inSingle) {
			inDouble = !inDouble;
			continue;
		}

		if (/\s/.test(c) && !inSingle && !inDouble) {
			if (current.length > 0) {
				tokens.push(current);
				current = '';
			}
			continue;
		}

		current += c;
	}

	if (current.length > 0) {
		tokens.push(current);
	}

	return tokens;
}

/**
 * Extract placeholders matching {{param}} or {param} (for LLM agent arguments)
 */
function extractLlmVariables(texts: string[]): string[] {
	const vars = new Set<string>();
	const regexDouble = /\{\{([a-zA-Z0-9_]+)\}\}/g;

	for (const text of texts) {
		let match: RegExpExecArray | null;
		while ((match = regexDouble.exec(text)) !== null) {
			vars.add(match[1]);
		}
	}
	return Array.from(vars);
}

/**
 * Extract session variables matching $VAR or ${VAR} (for shared plugin session state)
 */
function extractSessionVariables(texts: string[]): string[] {
	const vars = new Set<string>();
	const regexDollar = /\$([a-zA-Z0-9_]+)|\$\{([a-zA-Z0-9_]+)\}/g;

	for (const text of texts) {
		let match: RegExpExecArray | null;
		while ((match = regexDollar.exec(text)) !== null) {
			const v = match[1] || match[2];
			if (v && v !== 'COOKIES') {
				vars.add(v);
			}
		}
	}
	return Array.from(vars);
}

/**
 * Parses a raw cURL command (or variable assignment like COOKIES=$(curl ...))
 * into a safe, structured tool definition.
 */
export function parseCurlCommand(rawCmd: string, existingToolIndex = 0): ParsedCurlTool {
	let cleaned = rawCmd.trim();

	// 1. Detect assignment pattern: COOKIES=$(curl ...) or TOKEN=$(curl ...)
	let capturedVar: string | undefined = undefined;
	const assignMatch = cleaned.match(/^([a-zA-Z0-9_]+)\s*=\s*\$\(([\s\S]+)\)\s*$/);
	if (assignMatch) {
		capturedVar = assignMatch[1];
		cleaned = assignMatch[2].trim();
	}

	// Remove continuation backslashes at line endings
	cleaned = cleaned.replace(/\\\r?\n/g, ' ');

	const tokens = tokenizeArgs(cleaned);

	let method = 'GET';
	let url = '';
	const headers: Record<string, string> = {};
	let body: string | undefined = undefined;
	let cookieExpression: string | undefined = undefined;
	let capturesCookieVar = capturedVar === 'COOKIES' ? 'COOKIES' : undefined;

	let i = 0;
	// Skip leading "curl" if present
	if (tokens[0]?.toLowerCase() === 'curl') {
		i = 1;
	}

	while (i < tokens.length) {
		const token = tokens[i];

		if (token === '-X' || token === '--request') {
			method = tokens[i + 1]?.toUpperCase() || 'GET';
			i += 2;
			continue;
		}

		if (token === '-H' || token === '--header') {
			const headerStr = tokens[i + 1] || '';
			const sepIdx = headerStr.indexOf(':');
			if (sepIdx > -1) {
				const key = headerStr.substring(0, sepIdx).trim();
				const val = headerStr.substring(sepIdx + 1).trim();
				headers[key] = val;
			}
			i += 2;
			continue;
		}

		if (
			token === '-d' ||
			token === '--data' ||
			token === '--data-raw' ||
			token === '--data-binary' ||
			token === '--data-ascii'
		) {
			body = tokens[i + 1] || '';
			if (method === 'GET') {
				method = 'POST'; // curl defaults to POST when data is passed
			}
			i += 2;
			continue;
		}

		if (token === '-b' || token === '--cookie') {
			cookieExpression = tokens[i + 1] || '';
			i += 2;
			continue;
		}

		if (token === '-c' || token === '--cookie-jar') {
			capturesCookieVar = 'COOKIES';
			i += 2;
			continue;
		}

		if (token === '-u' || token === '--user') {
			const userPass = tokens[i + 1] || '';
			if (typeof btoa !== 'undefined') {
				headers['Authorization'] = `Basic ${btoa(userPass)}`;
			}
			i += 2;
			continue;
		}

		// Flags with no arguments to ignore safely
		if (
			token === '-s' ||
			token === '--silent' ||
			token === '-k' ||
			token === '--insecure' ||
			token === '-v' ||
			token === '--verbose' ||
			token === '-L' ||
			token === '--location' ||
			token === '-i' ||
			token === '--include'
		) {
			i += 1;
			continue;
		}

		// First non-flag token is likely the URL
		if (!token.startsWith('-') && !url) {
			url = token;
			i += 1;
			continue;
		}

		i += 1;
	}

	// Clean quotes from URL if any
	url = url.replace(/^['"]|['"]$/g, '');

	// Detect LLM variables {{param}}
	const textsToScan = [url, body || '', cookieExpression || '', ...Object.values(headers)];
	const rawLlmVars = extractLlmVariables(textsToScan);
	const rawSessionVars = extractSessionVariables(textsToScan);

	if (cookieExpression?.includes('$COOKIES') || cookieExpression === '$COOKIES') {
		rawSessionVars.push('COOKIES');
	}

	// Auto-generate tool name from method + url
	let toolName = `tool_${existingToolIndex + 1}`;
	try {
		if (url) {
			const u = new URL(url.replace(/\{\{[^}]+\}\}/g, 'param'));
			const pathParts = u.pathname.split('/').filter(Boolean);
			const lastPart = pathParts[pathParts.length - 1] || 'root';
			const cleanName = lastPart.replace(/[^a-zA-Z0-9_]/g, '_').toLowerCase();
			toolName = `${method.toLowerCase()}_${cleanName}`;
		}
	} catch {
		toolName = `${method.toLowerCase()}_action_${existingToolIndex + 1}`;
	}

	const variables: ParsedVariable[] = rawLlmVars.map((v) => ({
		name: v,
		type: 'string',
		description: `Parameter '${v}' for ${method} request`,
		required: true
	}));

	const properties: Record<string, any> = {};
	for (const v of variables) {
		properties[v.name] = {
			type: v.type,
			description: v.description
		};
	}

	return {
		name: toolName,
		description: `Execute ${method} request to ${url || 'endpoint'}`,
		method,
		url,
		headers,
		body,
		cookie_expression: cookieExpression,
		captures: {
			cookies: capturesCookieVar
		},
		variables,
		session_variables: Array.from(new Set(rawSessionVars)),
		input_schema: {
			type: 'object',
			properties,
			required: variables.filter((v) => v.required).map((v) => v.name)
		}
	};
}
