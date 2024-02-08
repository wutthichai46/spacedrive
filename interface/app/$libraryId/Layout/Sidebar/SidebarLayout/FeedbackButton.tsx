import clsx from 'clsx';
import { Controller } from 'react-hook-form';
import { auth, useBridgeMutation, useZodForm } from '@sd/client';
import { Button, Form, Popover, TextAreaField, toast, usePopover, z } from '@sd/ui';
import i18n from '~/app/I18n';
import { LoginButton } from '~/components/LoginButton';
import { useLocale } from '~/hooks';

const schema = z.object({
	message: z.string().min(1, { message: i18n.t('feedback_is_required') }),
	emoji: z.number().min(0).max(3)
});

const EMOJIS = ['🤩', '😀', '🙁', '😭'];

export default function () {
	const { t } = useLocale();

	const sendFeedback = useBridgeMutation(['api.sendFeedback'], {
		onError() {
			toast.error(t('feedback_toast_error_message'));
		},
		onSuccess() {
			toast.success(t('thank_you_for_your_feedback'));
		}
	});

	const form = useZodForm({
		schema,
		defaultValues: {
			emoji: -1,
			message: ''
		}
	});
	const popover = usePopover();

	const authState = auth.useStateSnapshot();

	return (
		<Popover
			popover={popover}
			trigger={
				<Button variant="outline" className="flex items-center gap-1">
					<p className="text-[11px] font-normal text-sidebar-inkFaint">{t('feedback')}</p>
				</Button>
			}
		>
			<Form
				form={form}
				onSubmit={form.handleSubmit(async (data) => {
					await sendFeedback.mutateAsync(data);
					form.reset();
					popover.setOpen(false);
				})}
				className="p-2"
			>
				<div className="flex w-72 flex-col gap-2">
					{authState.status !== 'loggedIn' && (
						<div className="flex flex-row items-center gap-2">
							<p className="flex-1 text-xs text-ink-dull">
								{authState.status !== 'loggingIn' &&
									t('feedback_login_description')}
							</p>
							<LoginButton cancelPosition="left" />
						</div>
					)}
					<TextAreaField
						{...form.register('message')}
						placeholder={t('feedback_placeholder')}
						className="!h-36 w-full flex-1"
					/>
					<div className="flex flex-row justify-between">
						<Controller
							control={form.control}
							name="emoji"
							render={({ field }) => (
								<div className="flex items-center justify-center gap-1 text-lg">
									{EMOJIS.map((emoji, i) => (
										<button
											type="button"
											onClick={() => field.onChange(i)}
											key={i}
											className={clsx(
												field.value === i ? 'bg-accent' : 'bg-app-input',
												'flex h-7 w-7 cursor-pointer items-center justify-center rounded-full border border-app-line transition-all duration-200 hover:scale-125'
											)}
										>
											{emoji}
										</button>
									))}
								</div>
							)}
						/>

						<Button type="submit" variant="accent" disabled={!form.formState.isValid}>
							{t('send')}
						</Button>
					</div>
				</div>
			</Form>
		</Popover>
	);
}
