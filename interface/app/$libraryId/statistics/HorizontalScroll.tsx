import { ArrowLeft, ArrowRight } from '@phosphor-icons/react';
import clsx from 'clsx';
import { ReactNode, useEffect, useRef, useState } from 'react';
import { useDraggable } from 'react-use-draggable-scroll';
import { tw } from '@sd/ui';

const ArrowButton = tw.div`absolute top-1/2 z-40 flex h-8 w-8 shrink-0 -translate-y-1/2 items-center p-2 cursor-pointer justify-center rounded-full border border-app-line bg-app/50 hover:opacity-95 backdrop-blur-md transition-all duration-200`;

export const useHorizontalScroll = () => {
	const ref = useRef<HTMLDivElement>(null);
	const { events } = useDraggable(ref as React.MutableRefObject<HTMLDivElement>);
	const [lastItemVisible, setLastItemVisible] = useState(false);
	const [scroll, setScroll] = useState(0);

	const updateScrollState = () => {
		const element = ref.current;
		if (element) {
			setScroll(element.scrollLeft);
			setLastItemVisible(element.scrollWidth - element.clientWidth === element.scrollLeft);
		}
	};

	useEffect(() => {
		const element = ref.current;
		if (element) {
			element.addEventListener('scroll', updateScrollState);
		}
		return () => {
			if (element) {
				element.removeEventListener('scroll', updateScrollState);
			}
		};
	}, [ref]);

	const handleArrowOnClick = (direction: 'right' | 'left') => {
		const element = ref.current;
		if (!element) return;

		element.scrollTo({
			left: direction === 'left' ? element.scrollLeft - 200 : element.scrollLeft + 200,
			behavior: 'smooth'
		});
	};

	return { ref, events, handleArrowOnClick, lastItemVisible, scroll };
};

export const HorizontalScroll = ({ children }: { children: ReactNode }) => {
	const { ref, events, handleArrowOnClick, lastItemVisible, scroll } = useHorizontalScroll();

	const maskImage = `linear-gradient(90deg, transparent 0.1%, rgba(0, 0, 0, 1) ${
		scroll > 0 ? '10%' : '0%'
	}, rgba(0, 0, 0, 1) ${lastItemVisible ? '95%' : '85%'}, transparent 99%)`;

	return (
		<div className="relative mb-4 flex pl-7">
			<ArrowButton
				onClick={() => handleArrowOnClick('right')}
				className={clsx('left-3', scroll === 0 && 'pointer-events-none opacity-0')}
			>
				<ArrowLeft weight="bold" className="h-4 w-4 text-ink" />
			</ArrowButton>
			<div
				ref={ref}
				{...events}
				className="no-scrollbar flex gap-2 space-x-px overflow-x-scroll pl-1 pr-[60px]"
				style={{
					WebkitMaskImage: maskImage,
					maskImage
				}}
			>
				{children}
			</div>

			<ArrowButton
				onClick={() => handleArrowOnClick('left')}
				className={clsx('right-3', lastItemVisible && 'pointer-events-none opacity-0')}
			>
				<ArrowRight weight="bold" className="h-4 w-4 text-ink" />
			</ArrowButton>
		</div>
	);
};
