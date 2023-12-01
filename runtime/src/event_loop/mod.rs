/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/.
 */

use std::collections::VecDeque;
use std::ffi::c_void;
use std::task;
use std::task::Poll;

use futures::future::poll_fn;
use mozjs::jsapi::{Handle, JSContext, JSObject, PromiseRejectionHandlingState};

use ion::{Context, ErrorReport, Object, Promise};
use ion::format::{Config, format_value};

use crate::ContextExt;
use crate::event_loop::future::FutureQueue;
use crate::event_loop::macrotasks::MacrotaskQueue;
use crate::event_loop::microtasks::MicrotaskQueue;

pub(crate) mod future;
pub(crate) mod macrotasks;
pub(crate) mod microtasks;

#[derive(Default)]
pub struct EventLoop {
	pub(crate) futures: Option<FutureQueue>,
	pub(crate) microtasks: Option<MicrotaskQueue>,
	pub(crate) macrotasks: Option<MacrotaskQueue>,
	pub(crate) unhandled_rejections: VecDeque<Object>,
}

impl EventLoop {
	pub async fn run_event_loop(&mut self, cx: &Context) -> Result<(), Option<ErrorReport>> {
		let mut complete = false;
		poll_fn(|wcx| self.poll_event_loop(cx, wcx, &mut complete)).await
	}

	fn poll_event_loop(
		&mut self, cx: &Context, wcx: &mut task::Context, complete: &mut bool,
	) -> Poll<Result<(), Option<ErrorReport>>> {
		if let Some(futures) = &mut self.futures {
			if !futures.is_empty() {
				futures.run_futures(cx, wcx)?;
			}
		}

		if let Some(microtasks) = &mut self.microtasks {
			if !microtasks.is_empty() {
				microtasks.run_jobs(cx)?;
			}
		}

		if let Some(macrotasks) = &mut self.macrotasks {
			if !macrotasks.is_empty() {
				macrotasks.run_jobs(cx)?;
			}
		}

		while let Some(promise) = self.unhandled_rejections.pop_front() {
			let promise = Promise::from(promise.into_root()).unwrap();
			let result = promise.result(cx);
			eprintln!(
				"Unhandled Promise Rejection: {}",
				format_value(cx, Config::default(), &result)
			);
		}

		let empty = self.is_empty();
		if empty && *complete {
			Poll::Ready(Ok(()))
		} else {
			wcx.waker().wake_by_ref();
			*complete = empty;
			Poll::Pending
		}
	}

	fn is_empty(&self) -> bool {
		self.microtasks.as_ref().map(|m| m.is_empty()).unwrap_or(true)
			&& self.futures.as_ref().map(|f| f.is_empty()).unwrap_or(true)
			&& self.macrotasks.as_ref().map(|m| m.is_empty()).unwrap_or(true)
	}
}

pub(crate) unsafe extern "C" fn promise_rejection_tracker_callback(
	cx: *mut JSContext, _: bool, promise: Handle<*mut JSObject>, state: PromiseRejectionHandlingState, _: *mut c_void,
) {
	let cx = unsafe { &Context::new_unchecked(cx) };
	let promise = Promise::from(cx.root(promise.get())).unwrap();
	let unhandled = unsafe { &mut (*cx.get_private().as_ptr()).event_loop.unhandled_rejections };
	match state {
		PromiseRejectionHandlingState::Unhandled => unhandled.push_back(Object::from(cx.root(promise.get()))),
		PromiseRejectionHandlingState::Handled => {
			let idx = unhandled.iter().position(|unhandled| unhandled.handle().get() == promise.get());
			if let Some(idx) = idx {
				unhandled.swap_remove_back(idx);
			}
		}
	}
}
