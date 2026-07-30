//! The client end of the connection.
//!
//! Everything the server says outwards goes through `Client`: responses to
//! requests, diagnostics, log lines, and pop-up messages. `Stdio` writes them to
//! the real LSP connection; `Recorder` keeps them in memory so a test can drive
//! a handler and read back what the client would have been told.

use lsp_server::{Connection, Message, Notification, RequestId, Response};
use lsp_types::{
	Diagnostic, LogMessageParams, MessageType, PublishDiagnosticsParams, ShowMessageParams, Uri,
	notification::{LogMessage, Notification as _, PublishDiagnostics, ShowMessage},
};

/// What the server can say to its client.
///
/// The two methods at the top are all an implementation has to provide; the
/// rest are the LSP messages this server actually sends, spelled out once here
/// so no handler has to assemble a `Notification` by hand.
pub trait Client {
	/// Answer a request. Every request is answered exactly once.
	fn send_response(&self, id: RequestId, result: serde_json::Value);

	/// Send a notification, which the client never answers.
	fn send_notification(&self, method: &str, params: serde_json::Value);

	fn respond<T: serde::Serialize>(&self, id: RequestId, result: T)
	where
		Self: Sized,
	{
		self.send_response(id, serde_json::to_value(result).unwrap());
	}

	/// Replace the diagnostics the client holds for `uri`. An empty list clears
	/// them.
	fn publish_diagnostics(&self, uri: Uri, diagnostics: Vec<Diagnostic>) {
		let params = PublishDiagnosticsParams {
			uri,
			diagnostics,
			version: None,
		};
		self.send_notification(
			PublishDiagnostics::METHOD,
			serde_json::to_value(params).unwrap(),
		);
	}

	/// Write to the client's log pane.
	fn log(&self, typ: MessageType, message: &str) {
		let params = LogMessageParams {
			typ,
			message: message.to_string(),
		};
		self.send_notification(LogMessage::METHOD, serde_json::to_value(params).unwrap());
	}

	/// Put a message in front of the user. Reserved for failures they asked for
	/// — a format that could not be applied, say.
	fn show_message(&self, typ: MessageType, message: &str) {
		let params = ShowMessageParams {
			typ,
			message: message.to_string(),
		};
		self.send_notification(ShowMessage::METHOD, serde_json::to_value(params).unwrap());
	}
}

/// The client on the other end of stdin/stdout.
///
/// Sends are best-effort: once the client has hung up there is nothing useful
/// left to do about a failed write, and the message loop ends on its own when
/// the receiver closes.
pub struct Stdio<'a> {
	connection: &'a Connection,
}

impl<'a> Stdio<'a> {
	pub fn new(connection: &'a Connection) -> Self {
		Self { connection }
	}
}

impl Client for Stdio<'_> {
	fn send_response(&self, id: RequestId, result: serde_json::Value) {
		let response = Response {
			id,
			result: Some(result),
			error: None,
		};
		self.connection
			.sender
			.send(Message::Response(response))
			.ok();
	}

	fn send_notification(&self, method: &str, params: serde_json::Value) {
		let not = Notification::new(method.to_string(), params);
		self.connection.sender.send(Message::Notification(not)).ok();
	}
}

/// A client that remembers what it was told, for tests.
#[cfg(test)]
pub struct Recorder {
	messages: std::cell::RefCell<Vec<Message>>,
}

#[cfg(test)]
impl Recorder {
	pub fn new() -> Self {
		Self {
			messages: std::cell::RefCell::new(Vec::new()),
		}
	}

	/// The result of the first response, deserialized. `None` if nothing was
	/// answered.
	pub fn response<T: serde::de::DeserializeOwned>(&self) -> Option<T> {
		self.messages.borrow().iter().find_map(|msg| match msg {
			Message::Response(r) => serde_json::from_value(r.result.clone()?).ok(),
			_ => None,
		})
	}

	/// The params of every notification sent with `method`, deserialized.
	pub fn notifications<T: serde::de::DeserializeOwned>(&self, method: &str) -> Vec<T> {
		self.messages
			.borrow()
			.iter()
			.filter_map(|msg| match msg {
				Message::Notification(n) if n.method == method => {
					serde_json::from_value(n.params.clone()).ok()
				}
				_ => None,
			})
			.collect()
	}

	pub fn diagnostics(&self) -> Vec<PublishDiagnosticsParams> {
		self.notifications(PublishDiagnostics::METHOD)
	}

	pub fn logs(&self) -> Vec<String> {
		self.notifications::<LogMessageParams>(LogMessage::METHOD)
			.into_iter()
			.map(|p| p.message)
			.collect()
	}

	pub fn shown_messages(&self) -> Vec<String> {
		self.notifications::<ShowMessageParams>(ShowMessage::METHOD)
			.into_iter()
			.map(|p| p.message)
			.collect()
	}

	pub fn is_silent(&self) -> bool {
		self.messages.borrow().is_empty()
	}
}

#[cfg(test)]
impl Client for Recorder {
	fn send_response(&self, id: RequestId, result: serde_json::Value) {
		self.messages.borrow_mut().push(Message::Response(Response {
			id,
			result: Some(result),
			error: None,
		}));
	}

	fn send_notification(&self, method: &str, params: serde_json::Value) {
		self.messages
			.borrow_mut()
			.push(Message::Notification(Notification::new(
				method.to_string(),
				params,
			)));
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn uri() -> Uri {
		"file:///test.nsi".parse().unwrap()
	}

	#[test]
	fn a_fresh_recorder_is_silent() {
		assert!(Recorder::new().is_silent());
	}

	#[test]
	fn responses_come_back_typed() {
		let client = Recorder::new();
		client.respond(RequestId::from(1), vec!["one", "two"]);

		assert_eq!(
			client.response::<Vec<String>>(),
			Some(vec!["one".to_string(), "two".to_string()])
		);
	}

	/// A handler with nothing to say still answers, with null.
	#[test]
	fn a_null_response_is_still_a_response() {
		let client = Recorder::new();
		client.respond(RequestId::from(1), Option::<u32>::None);

		assert!(!client.is_silent());
		assert_eq!(client.response::<Option<u32>>(), Some(None));
	}

	#[test]
	fn diagnostics_carry_their_uri() {
		let client = Recorder::new();
		client.publish_diagnostics(uri(), vec![Diagnostic::default()]);

		let published = client.diagnostics();
		assert_eq!(published.len(), 1);
		assert_eq!(published[0].uri, uri());
		assert_eq!(published[0].diagnostics.len(), 1);
	}

	#[test]
	fn logs_and_shown_messages_stay_apart() {
		let client = Recorder::new();
		client.log(MessageType::INFO, "to the log");
		client.show_message(MessageType::ERROR, "to the user");

		assert_eq!(client.logs(), vec!["to the log"]);
		assert_eq!(client.shown_messages(), vec!["to the user"]);
	}
}
