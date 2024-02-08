import { DotsThreeOutlineVertical } from 'phosphor-react-native';
import { Text, View } from 'react-native';
import { AnimatedCircularProgress } from 'react-native-circular-progress';
import { ScrollView } from 'react-native-gesture-handler';
import { tw } from '~/lib/tailwind';

import FolderIcon from '../icons/FolderIcon';
import Fade from '../layout/Fade';

const Jobs = () => {
	return (
		<View style={tw`gap-5`}>
			<View style={tw`w-full flex-row items-center justify-between px-7`}>
				<Text style={tw`text-lg font-bold text-white`}>Jobs</Text>
			</View>
			<Fade color="mobile-screen" height="100%" width={30}>
				<ScrollView horizontal showsHorizontalScrollIndicator={false}>
					<View style={tw`flex-row gap-2 px-7`}>
						<Job message="Processed 300 of 1431 orphan paths..." progress={55} />
						<Job message="All tasks have been completed successfully" progress={100} />
						<Job
							message="An error has occurred while adding location"
							error
							progress={100}
						/>
					</View>
				</ScrollView>
			</Fade>
		</View>
	);
};

interface JobProps {
	progress: number;
	message: string;
	error?: boolean;
	// job: JobReport // to be added latter
}

const Job = ({ progress, message, error }: JobProps) => {
	const progressColor = error
		? tw.color('red-500')
		: progress === 100
		? tw.color('green-500')
		: tw.color('accent');
	return (
		<View
			style={tw`h-[170px] w-[310px] flex-col rounded-md border border-sidebar-line/50 bg-sidebar-box`}
		>
			<View
				style={tw`w-full flex-row items-center justify-between rounded-t-md border-b border-sidebar-line/80 bg-mobile-header/50 px-5 py-2`}
			>
				<View style={tw`flex-row items-center gap-2`}>
					<FolderIcon size={36} />
					<Text style={tw`text-md font-bold text-white`}>Added Memories</Text>
				</View>
				<DotsThreeOutlineVertical weight="fill" size={20} color={tw.color('ink-faint')} />
			</View>
			<View style={tw`mx-auto flex-1 flex-row items-center justify-between gap-5 px-5 py-3`}>
				<AnimatedCircularProgress
					size={80}
					width={7}
					fill={progress}
					rotation={0}
					prefill={error ? 100 : 0}
					lineCap="round"
					tintColor={progressColor}
					backgroundColor={tw.color('ink-light/5')}
				>
					{(fill) => (
						<View style={tw`flex-row items-end gap-[1px]`}>
							<Text style={tw`text-lg font-bold text-white`}>
								{error ? '0' : fill.toFixed(0)}
							</Text>
							<Text
								style={tw`relative bottom-[6px] text-[10px] font-bold text-white`}
							>
								{'%'}
							</Text>
						</View>
					)}
				</AnimatedCircularProgress>
				<Text style={tw`w-[60%] text-sm leading-5 text-ink-dull`}>{message}</Text>
			</View>
		</View>
	);
};

export default Jobs;
