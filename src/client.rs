use std::io::{self, Read};

use hyper::client::IntoUrl;
use hyper::header::{Headers, ContentType, Location, Referer, UserAgent};
use hyper::method::Method;
use hyper::status::StatusCode;
use hyper::version::HttpVersion;
use hyper::{Url};

use serde::Serialize;
use serde_json;
use serde_urlencoded;

use ::body::{self, Body};

static DEFAULT_USER_AGENT: &'static str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

/// A `Client` to make Requests with.
///
/// The Client has various configuration values to tweak, but the defaults
/// are set to what is usually the most commonly desired value.
///
/// The `Client` holds a connection pool internally, so it is advised that
/// you create one and reuse it.
#[derive(Debug)]
pub struct Client {
    inner: ::hyper::Client,
}

impl Client {
    /// Constructs a new `Client`.
    pub fn new() -> ::Result<Client> {
        let mut client = try!(new_hyper_client());
        client.set_redirect_policy(::hyper::client::RedirectPolicy::FollowNone);
        Ok(Client {
            inner: client
        })
    }

    /// Convenience method to make a `GET` request to a URL.
    pub fn get<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::Get, url)
    }

    /// Convenience method to make a `POST` request to a URL.
    pub fn post<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::Post, url)
    }

    /// Convenience method to make a `HEAD` request to a URL.
    pub fn head<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::Head, url)
    }

    /// Start building a `Request` with the `Method` and `Url`.
    ///
    /// Returns a `RequestBuilder`, which will allow setting headers and
    /// request body before sending.
    pub fn request<U: IntoUrl>(&self, method: Method, url: U) -> RequestBuilder {
        let url = url.into_url();
        RequestBuilder {
            client: self,
            method: method,
            url: url,
            _version: HttpVersion::Http11,
            headers: Headers::new(),

            body: None,
        }
    }
}

fn new_hyper_client() -> ::Result<::hyper::Client> {
    use tls::TlsClient;
    Ok(::hyper::Client::with_connector(
        ::hyper::client::Pool::with_connector(
            Default::default(),
            ::hyper::net::HttpsConnector::new(try!(TlsClient::new()))
        )
    ))
}


/// A builder to construct the properties of a `Request`.
#[derive(Debug)]
pub struct RequestBuilder<'a> {
    client: &'a Client,

    method: Method,
    url: Result<Url, ::UrlError>,
    _version: HttpVersion,
    headers: Headers,

    body: Option<::Result<Body>>,
}

impl<'a> RequestBuilder<'a> {
    /// Add a `Header` to this Request.
    ///
    /// ```no_run
    /// use reqwest::header::UserAgent;
    /// let client = reqwest::Client::new().expect("client failed to construct");
    ///
    /// let res = client.get("https://www.rust-lang.org")
    ///     .header(UserAgent("foo".to_string()))
    ///     .send();
    /// ```
    pub fn header<H: ::header::Header + ::header::HeaderFormat>(mut self, header: H) -> RequestBuilder<'a> {
        self.headers.set(header);
        self
    }
    /// Add a set of Headers to the existing ones on this Request.
    ///
    /// The headers will be merged in to any already set.
    pub fn headers(mut self, headers: ::header::Headers) -> RequestBuilder<'a> {
        self.headers.extend(headers.iter());
        self
    }

    /// Set the request body.
    pub fn body<T: Into<Body>>(mut self, body: T) -> RequestBuilder<'a> {
        self.body = Some(Ok(body.into()));
        self
    }

    /// Send a form body.
    ///
    /// Sets the body to the url encoded serialization of the passed value,
    /// and also sets the `Content-Type: application/www-form-url-encoded`
    /// header.
    ///
    /// ```no_run
    /// # use std::collections::HashMap;
    /// let mut params = HashMap::new();
    /// params.insert("lang", "rust");
    ///
    /// let client = reqwest::Client::new().unwrap();
    /// let res = client.post("http://httpbin.org")
    ///     .form(&params)
    ///     .send();
    /// ```
    pub fn form<T: Serialize>(mut self, form: &T) -> RequestBuilder<'a> {
        let body = serde_urlencoded::to_string(form).map_err(::Error::from);
        self.headers.set(ContentType::form_url_encoded());
        self.body = Some(body.map(|b| b.into()));
        self
    }

    /// Send a JSON body.
    ///
    /// Sets the body to the JSON serialization of the passed value, and
    /// also sets the `Content-Type: application/json` header.
    ///
    /// ```no_run
    /// # use std::collections::HashMap;
    /// let mut map = HashMap::new();
    /// map.insert("lang", "rust");
    ///
    /// let client = reqwest::Client::new().unwrap();
    /// let res = client.post("http://httpbin.org")
    ///     .json(&map)
    ///     .send();
    /// ```
    pub fn json<T: Serialize>(mut self, json: &T) -> RequestBuilder<'a> {
        let body = serde_json::to_vec(json).expect("serde to_vec cannot fail");
        self.headers.set(ContentType::json());
        self.body = Some(Ok(body.into()));
        self
    }

    /// Constructs the Request and sends it the target URL, returning a Response.
    pub fn send(mut self) -> ::Result<Response> {
        if !self.headers.has::<UserAgent>() {
            self.headers.set(UserAgent(DEFAULT_USER_AGENT.to_owned()));
        }

        let client = self.client;
        let mut method = self.method;
        let mut url = try!(self.url);
        let mut headers = self.headers;
        let mut body = match self.body {
            Some(b) => Some(try!(b)),
            None => None,
        };

        let mut redirect_count = 0;

        loop {
            let res = {
                debug!("request {:?} \"{}\"", method, url);
                let mut req = client.inner.request(method.clone(), url.clone())
                    .headers(headers.clone());

                if let Some(ref mut b) = body {
                    let body = body::as_hyper_body(b);
                    req = req.body(body);
                }

                try!(req.send())
            };
            body.take();

            match res.status {
                StatusCode::MovedPermanently |
                StatusCode::Found |
                StatusCode::SeeOther => {

                    //TODO: turn this into self.redirect_policy.check()
                    if redirect_count > 10 {
                        return Err(::Error::TooManyRedirects);
                    }
                    redirect_count += 1;

                    method = match method {
                        Method::Post | Method::Put => Method::Get,
                        m => m
                    };

                    headers.set(Referer(url.to_string()));

                    let loc = {
                        let loc = res.headers.get::<Location>().map(|loc| url.join(loc));
                        if let Some(loc) = loc {
                            loc
                        } else {
                            return Ok(Response {
                                inner: res
                            });
                        }
                    };

                    url = match loc {
                        Ok(u) => u,
                        Err(e) => {
                            debug!("Location header had invalid URI: {:?}", e);
                            return Ok(Response {
                                inner: res
                            })
                        }
                    };

                    debug!("redirecting to '{}'", url);

                    //TODO: removeSensitiveHeaders(&mut headers, &url);

                },
                _ => {
                    return Ok(Response {
                        inner: res
                    });
                }
            }
        }
    }
}

/// A Response to a submitted `Request`.
#[derive(Debug)]
pub struct Response {
    inner: ::hyper::client::Response,
}

impl Response {
    /// Get the `StatusCode`.
    pub fn status(&self) -> &StatusCode {
        &self.inner.status
    }

    /// Get the `Headers`.
    pub fn headers(&self) -> &Headers {
        &self.inner.headers
    }

    /// Get the `HttpVersion`.
    pub fn version(&self) -> &HttpVersion {
        &self.inner.version
    }
}

/// Read the body of the Response.
impl Read for Response {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}
