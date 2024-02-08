import { useCallback, useEffect, useRef, useState } from 'react';
import { useNavigate } from 'react-router';
import { createSearchParams } from 'react-router-dom';
import { useDebouncedCallback } from 'use-debounce';
import { Input, ModifierKeys, Shortcut } from '@sd/ui';
import { useOperatingSystem } from '~/hooks';
import { keybindForOs } from '~/util/keybinds';

import { useSearchContext } from './context';
import { useSearchStore } from './store';

interface Props {
	redirectToSearch?: boolean;
}

export default ({ redirectToSearch }: Props) => {
	const search = useSearchContext();
	const searchRef = useRef<HTMLInputElement>(null);
	const navigate = useNavigate();
	const searchStore = useSearchStore();

	const os = useOperatingSystem(true);
	const keybind = keybindForOs(os);

	const focusHandler = useCallback(
		(event: KeyboardEvent) => {
			if (
				event.key.toUpperCase() === 'F' &&
				event.getModifierState(os === 'macOS' ? ModifierKeys.Meta : ModifierKeys.Control)
			) {
				event.preventDefault();
				searchRef.current?.focus();
			}
		},
		[os]
	);

	const blurHandler = useCallback((event: KeyboardEvent) => {
		if (event.key === 'Escape' && document.activeElement === searchRef.current) {
			// Check if element is in focus, then remove it
			event.preventDefault();
			searchRef.current?.blur();
		}
	}, []);

	useEffect(() => {
		const input = searchRef.current;
		document.body.addEventListener('keydown', focusHandler);
		input?.addEventListener('keydown', blurHandler);
		return () => {
			document.body.removeEventListener('keydown', focusHandler);
			input?.removeEventListener('keydown', blurHandler);
		};
	}, [blurHandler, focusHandler]);

	const [value, setValue] = useState('');

	useEffect(() => {
		setValue(search.rawSearch);
	}, [search.rawSearch]);

	const updateDebounce = useDebouncedCallback((value: string) => {
		search.setSearch(value);
		if (redirectToSearch) {
			navigate({
				pathname: '../search',
				search: createSearchParams({
					search: value
				}).toString()
			});
		}
	}, 300);

	function updateValue(value: string) {
		setValue(value);
		updateDebounce(value);
	}

	function clearValue() {
		search.setSearch('');
	}

	return (
		<Input
			ref={searchRef}
			placeholder="Search"
			className="w-48 mx-2 transition-all duration-200 focus-within:w-60"
			size="sm"
			value={value}
			onChange={(e) => {
				updateValue(e.target.value);
			}}
			onBlur={() => {
				if (search.rawSearch === '' && !searchStore.interactingWithSearchOptions) {
					clearValue();
					search.setSearchBarFocused(false);
				}
			}}
			onFocus={() => search.setSearchBarFocused(true)}
			right={
				<div className="flex items-center space-x-1 pointer-events-none h-7 opacity-70 group-focus-within:hidden">
					{
						<Shortcut
							chars={keybind([ModifierKeys.Control], ['F'])}
							aria-label={`Press ${
								os === 'macOS' ? 'Command' : ModifierKeys.Control
							}-F to focus search bar`}
							className="border-none"
						/>
					}
				</div>
			}
		/>
	);
};
