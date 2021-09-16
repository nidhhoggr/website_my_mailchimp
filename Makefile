all: clean
	rustfmt src/main.rs
	cargo build
	cp $(PWD)/target/debug/website_my_mailchimp $(PWD)/
	$(PWD)/website_my_mailchimp

release: clean
	rustfmt src/main.rs
	cargo build --release
	cp $(PWD)/target/release/website_my_mailchimp $(PWD)/
	$(PWD)/website_my_mailchimp

clean:
	rm -rf $(PWD)/target/debug/dist/
	rm -rf $(PWD)/target/debug/scraped/
	rm -rf $(PWD)/dist/
	rm -rf $(PWD)/scraped/
